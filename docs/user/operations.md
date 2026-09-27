# Operations

This document is the runbook. It covers what the daemon requires, what the events
mean, and what to do when something is wrong. It grows as the daemon gains
behavior; every transition reason listed here MUST have an entry
(`R-17`, `R-33`).

## Requirements

The daemon requires `CAP_NET_ADMIN` and `CAP_NET_RAW`, and refuses to start
without them rather than running a VRRP router that cannot move an address.

- Linux with `CAP_NET_ADMIN` and `CAP_NET_RAW`.
- The configuration file readable by the daemon, and not world-writable.
- The control socket directory writable at startup.
- Multicast mode additionally needs the VRRP multicast group to reach the
  segment; firewall rules are the operator's responsibility, and Highland never
  modifies them.

## Running

```console
$ highland run --config /etc/highland/config.toml
$ highland status
$ highland status --json
$ highland show api
$ highland events --follow
$ highland reload
$ highland relinquish api --yes
```

Destructive commands name the target instance and its current role, and
`--yes` is accepted so a script can be written without a prompt. The
confirmation prompt itself is not implemented yet, so treat `--yes` as what it
is: a way to say "I meant it". Every command produces an audit event naming the
peer credential (`R-28`), and a refusal exits non-zero.

`force-transition` is disabled unless the daemon was started with
`--enable-force-transition` (`R-10`). It exists for breaking a stuck state
machine, and it is the first thing to look suspicious in an incident. It refuses
without an explicit confirmation, and the refusal is a non-zero exit.

## Signals

| Signal | Effect |
|---|---|
| `SIGTERM`, `SIGINT` | Graceful shutdown (`SPEC.md` §14.5) |
| `SIGHUP` | Applies the configuration, or refuses the whole reload |
| A second `SIGTERM` | Ignored; shutdown stays idempotent (`I-31`) |

`SIGHUP` reports `reload_accepted` or `reload_rejected`, and a rejected reload
leaves the running configuration untouched. It is the same operation as
`highland reload` — one implementation, shared — so the two cannot disagree
about what a reload does.

Shutdown order: stop accepting control requests, stop health checks, send a
zero-priority advertisement and remove VIPs for each master, dump state, exit.
The default budget is five seconds; exceeding it is an error, and the daemon
names the addresses it could not remove.

## Metrics

Set `metrics.enabled = true` and a `metrics.listen` address, and the daemon
serves Prometheus text format there. Two rules matter operationally:

- `highland_instance_role` is numeric: `0` INIT, `1` BACKUP, `2` MASTER, `3`
  FAULT, `4` DISABLED (`R-19`).
- No metric label contains a peer address, an address, or an error string
  (`R-20`). Peer detail belongs in the event stream.

## Transition reasons

Every role transition carries one of these. This list is the closed set
(`R-17`); a new reason is a specification change. These are the exact strings
the code emits, so they are safe to alert on.

| Reason | Meaning | What to check |
|---|---|---|
| `startup` | The instance entered election | Expected at boot |
| `interface_up` | The interface became usable | Expected after a link event |
| `interface_down` | The interface became unusable | Check the link, the peer, and the switch |
| `master_down_timeout` | No advertisement arrived within `3 * adver_int + Skew_Time` | Packet loss, or the peer died |
| `preemption_delay_elapsed` | A higher-priority backup took over after its preemption delay | Expected during a rolling restart of a higher-priority node |
| `higher_priority_peer_advertisement` | A higher-priority peer advertised, so this master stepped down | Expected during preemption |
| `health_ineligible` | Health policy made the instance ineligible | Inspect the failing check |
| `operator_relinquish` | An operator asked for relinquish | Intentional |
| `operator_force_transition` | A forced transition | Investigate why it was needed |
| `configuration_reloaded` | A reload permitted the transition | Check the change plan |
| `hold_down_expired` | The hold-down expired and re-verification succeeded | A previous fault recovered |
| `ownership_failed` | Adding or removing a VIP failed | Read the kernel error |
| `shutdown` | The process is stopping | Expected |

## Failure playbook

### The VIP is not moving

1. `highland status` for both nodes. Is the backup `BACKUP`, and is its
   master-down timer counting down?
2. `highland events --follow` on both. Is the master still sending?
3. Capture the segment and confirm advertisements are arriving at the peer
   (`compatibility.md` has the capture procedure).
4. Check the firewall. VRRP uses IP protocol 112 and requires TTL 255.

### Two nodes both believe they are master

This is a layer-2 partition and VRRP cannot prevent it. See
[`compatibility.md`](compatibility.md) and the split-brain section below.
Record both `role_transition` events with their reasons, and fix the network.

### A node is stuck in FAULT

The instance failed to take or release ownership, or held a fault. Read the
`ownership_failed` event: it names the interface, the address, and the kernel
error. The daemon retries with bounded backoff and then stops; resume the
instance or reload the configuration.

### Health checks cause unexpected demotion

Under `fail_closed`, a single blocking check crossing its failure threshold
relinquishes immediately. Under `weighted`, priority drops and the node steps
down only when a higher-priority peer advertises. To make health advisory, set
`weight = 0` on the check, or set `failure_policy = "manual"`.

### A reload was rejected

The running configuration is unchanged. Run `highland check-config` on the file:
every violation is reported with its rule and key.

## Split brain

VRRP protects against a single node failing, not against a partition in which
both nodes can still reach clients. Highland:

- reports peer loss on both sides,
- makes no claim of consensus,
- exposes role history and peer state for inspection,
- offers `relinquish` and `no-preempt` as operational tools,
- does not offer fencing; that is post-1.0 and out of the election path
  entirely.

## Upgrade and rollback

1. `highland check-config` the new file on a staging host.
2. Install the new binaries.
3. `highland reload` on one node at a time, watching events.
4. If a node misbehaves, restore the previous binaries and reload. The previous
   configuration is retained in memory until the next successful reload
   (`I-11`).

Protocol behavior is standards-compliant across the 0.x line, so mixed-version
pairs interoperate during a rolling upgrade. Configuration changes require a
migration note in `CHANGELOG.md`.
