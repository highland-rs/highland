# Troubleshooting

A symptom-first guide. Find what you see, then work down to the cause.

Two habits pay for themselves before anything else:

- **Read the event stream.** Every role change carries a reason. Highland tells
  you why the address moved; the guess is already written down.
  `highland events --follow`, or `highland events --limit 50` after the fact.
- **Change one thing at a time.** Most bad outcomes in a failover system come
  from fixing two things and not knowing which one mattered.

## Contents

- [The daemon will not start](#the-daemon-will-not-start)
- [The address is not moving](#the-address-is-not-moving)
- [Both nodes think they own the address](#both-nodes-think-they-own-the-address)
- [A node is stuck in a fault state](#a-node-is-stuck-in-a-fault-state)
- [The address moved when nothing was wrong](#the-address-moved-when-nothing-was-wrong)
- [The address will not come back](#the-address-will-not-come-back)
- [Health checks fail but the service is fine](#health-checks-fail-but-the-service-is-fine)
- [The configuration is rejected](#the-configuration-is-rejected)
- [A reload was refused](#a-reload-was-refused)
- [I cannot reach the daemon](#i-cannot-reach-the-daemon)
- [Commands do nothing](#commands-do-nothing)
- [What to collect before asking for help](#what-to-collect-before-asking-for-help)

## The daemon will not start

**Symptom**:

```console
$ highland run --config /etc/highland/config.toml
ERROR highland: the VRRP transport is not implemented; the daemon will not start
```

This is expected today, and it is deliberate. Highland has no raw VRRP socket
yet, so the daemon declines to run rather than sit there claiming to be a VRRP
router while sending nothing. There is nothing to fix on your side.

Two things to know:

- **The configuration is still checked first.** A broken file reports the broken
  file, not this message. Run `highland check-config` to see which.
- **Do not install the systemd unit yet.** It has `Restart=on-failure`, so
  systemd would restart the daemon every two seconds indefinitely.

If you have an existing unit installed, stop and disable it:

```console
$ sudo systemctl disable --now highland.service
```

The test suite is where the failover is exercised in the meantime:

```console
$ cargo test -p highland-daemon --test failover
```

That drives a complete failover — startup, takeover, ownership confirmed before
advertising, relinquishment, shutdown — against a scripted kernel, with no
privileges required.

## The address is not moving

**Symptom**: one node is `MASTER`, the other is `BACKUP`, and the address never
changes when the master should fail.

1. **Is the backup counting down?** `highland show <instance>`. The
   `master_down_remaining_ms` field should be non-null on a backup and should
   fall. If it is `null`, the instance is not in a state where it expects the
   master to die.
2. **Is the master still advertising?** `highland events --follow` on the master,
   and on the backup, and watch for `master_down_timeout` on the backup. Its
   absence means advertisements are arriving.
3. **Are advertisements on the wire at all?**

   ```console
   $ sudo tcpdump -i <iface> -w highland.pcap 'ip proto 112'
   ```

   VRRP is IP protocol 112. If you see nothing, the daemon is not sending or
   your filter is wrong. If you see packets and the backup still does nothing,
   the packets are being rejected — check the next point.
4. **Are the peers configured to trust each other?** Each node must list the
   other. A node never accepts an advertisement from an address it was not told
   to accept, and it will not advertise to one either. This is the single most
   common misconfiguration, and it is silent: no error, no transition, just no
   failover.
5. **Is the firewall in the way?** VRRP requires IP protocol 112 and TTL 255.
   Highland does not modify firewall rules; if yours drops or rewrites protocol
   112, the address will never move. This is the second most common cause.
6. **Do both nodes use the same VRID and the same address?** If they differ, they
   are two unrelated groups and no amount of debugging will connect them.

## Both nodes think they own the address

**Symptom**: two `MASTER` nodes, or clients reaching the address
intermittently.

This is a network partition, and it is a property of layer-2 failover, not a bug.
Highland detects it, reports it, and does not pretend to resolve it. It has no
fencing and does not claim consensus.

What to do now:

1. Record the `role_transition` event and its reason on both nodes. That pair of
   records is the diagnosis.
2. Find the partition. It is almost always a link, a bridge, or a firewall rule
   that fails in one direction only. Asymmetric reachability is enough: a
   unidirectional break produces exactly this.
3. Decide which node should hold the address and remove the address from the
   other with `highland relinquish <instance> --yes`, or by taking that node out
   of service.

How to reduce the chance of it recurring:

- `preempt = false`, so a returning node waits instead of taking the address
  back and forth.
- `relinquish` during maintenance rather than killing a process.
- Network design that fails the peer link and the client link together, so the
  two nodes lose reachability at the same time.

External fencing and quorum are outside layer-2 failover entirely. See
[split-brain behavior](compatibility.md#split-brain-behavior).

## A node is stuck in a fault state

**Symptom**: an instance is `FAULT` and will not become master, or will not give
up an address it already holds.

A fault means an ownership operation failed: an address could not be added or
removed, or a failure was detected that the machine could not verify its way out
of. It is a deliberate refusal to guess.

1. Read the `ownership_failed` event. It names the interface, the address, and
   the kernel error. That is your cause.
2. Check the obvious kernel-level reasons:

   ```console
   $ ip -brief address show          # is the address already there?
   $ ip -brief link show             # is the link up and does it have carrier?
   $ sudo dmesg | tail -50           # address conflicts and rejections show up here
   ```
3. Check the address is not held somewhere you do not know about. A second
   machine, a bridge, or a stale entry from a previous run all produce the same
   kernel error.
4. Once the cause is fixed, the instance retries on its own with bounded
   backoff, and after a hold-down period it re-verifies the interface and the
   address before trying again. If you would rather not wait:
   `highland resume <instance>`, or reload the configuration.

## The address moved when nothing was wrong

**Symptom**: the address is flapping between nodes.

This is almost always a health check flickering, or a link flapping, and the
event stream says which.

1. Look for `master_down_timeout` and repeated `role_transition` events with the
   reason `higher_priority_peer_advertisement` or `health_ineligible`.
2. If health is involved, the event stream names the check. See
   [Health checks](health-checks.md) for how to make a marginal check stable:
   raise `failure_threshold`, raise `success_threshold` so recovery is slower
   than failure, and add `initial_grace_period` if it happens at boot.
3. If packet loss is involved, a check that times out counts as a failure. Loosen
   `timeout`, or lengthen `failure_threshold`.
4. If neither, look at the link. Counters on the interface and the switch tell
   you about a flapping port faster than anything else will.

A flapping address is worse for clients than a slow one. If you cannot identify
the cause quickly, set `preempt = false` and `failure_policy = "manual"` to stop
the movement, then investigate at leisure.

## The address will not come back

**Symptom**: a node is `BACKUP` and will not take the address even though its
peer is gone.

1. Is the node eligible? Eligibility requires the role not disabled, the
   interface up with carrier, `startup_delay` elapsed, and health permitting.
   `highland show <instance>` reports the health state; a node in a fault state
   is not eligible.
2. Is preemption configured against it? With `preempt = false`, a backup only
   takes over after the full master-down interval expires, not immediately when
   the master disappears.
3. Has it just started or resumed? A returning node waits out `startup_delay` and
   at least one health evaluation window before competing, on purpose.
4. Is the takeover delay what you expect? It is
   `3 × advertisement_interval + Skew_Time`, where RFC 5798 defines
   `Skew_Time` as `((256 − priority) / 256) × advertisement_interval`. At a
   one-second interval that is 3.41s behind a priority-150 master and 3.61s
   behind a priority-100 one, so between three and four intervals depending on
   priority. A higher-priority master is detected slightly sooner.

## Health checks fail but the service is fine

**Symptom**: `check_failing` events for a service that is serving normally.

1. Check what the probe actually asked for. An `http` check with
   `expected_status = [200]` fails on a `302`, and a redirect is often perfectly
   healthy.
2. Check for TLS problems on an `https` check: an untrusted certificate, a name
   mismatch, or a missing intermediate.
3. Check the timeout. A check whose `timeout` is close to its `interval` will
   report failures under load that are really latency.
4. For a `unix` or `file` check, check the path. It must be the path as the
   service sees it, and for `file` it must be inside the allowed base directory.
5. If the check is genuinely advisory, set `weight = 0`. It stays visible in the
   event stream and stops affecting the address.

To stop a check affecting the address at all, without editing it, set
`failure_policy = "manual"`.

## The configuration is rejected

**Symptom**: `check-config` or a reload refuses the file, with a rule reference
such as `V-01`.

The reference is the answer. Look it up in the
[configuration reference](configuration.md); each one names the exact condition.

The rules you will meet most:

| Reference | Cause |
|---|---|
| `V-01` | The VRID is 0. Use 1 to 255. |
| `V-02` | The priority is 0, which is reserved for relinquishment |
| `V-03` | The instance mixes IPv4 and IPv6 addresses, which 1.0 does not allow yet |
| `V-04` | The advertisement interval is outside 10ms to 40.95s |
| `V-06` | Two instances share an interface and VRID |
| `V-07` | An address family has no peer of that family in unicast mode |
| `V-08` | A peer you listed is this node's own address |
| `V-10` | `preempt_delay` is set while `preempt = false` |
| `V-13` | An address is malformed, or has a prefix of 0 |
| `V-14` | A check timeout exceeds its interval, or either is zero |
| `V-22` | The named interface does not exist on this machine |
| `V-26` | The configuration file is world-writable |

`V-26` has a deliberate override, `--allow-insecure-config`, for a file on a
trusted filesystem. Read [Installation](installation.md) before using it.

Two rules are checked by the daemon and not by `check-config`, because they
depend on the machine: `V-08` and `V-22`. If a file passes `check-config` on one
node and is rejected on another, one of those two is the reason.

## A reload was refused

Nothing changed. This is the design: a reload that cannot be fully applied is
refused in full, and the running configuration is left exactly as it was
(`I-09`).

Neither form of reload works yet, so in practice this is what you see when
`SIGHUP` finds a file that no longer validates: `reload_rejected` in the log, and
the running configuration untouched. Validate first with `highland check-config`
to see the violations before you signal.

1. The refusal names every rule the new file breaks. See
   [Upgrading](upgrading.md#what-can-be-reloaded-and-what-cannot) for which
   changes can be applied live and which need a restart.
2. Validate the new file: `highland check-config`. Every violation is reported
   with its rule and key.
3. If the change is legitimate but needs a restart, apply it to one instance at
   a time, during a window you have chosen.
4. If you are not sure what is running right now, ask the daemon; it retains the
   previous accepted configuration, so a diagnosis does not require guessing.

## I cannot reach the daemon

**Symptom**: a command fails to connect to the control socket.

- Is the daemon running? `systemctl status highland.service`.
- Is the socket where the command is looking? `--socket <path>`, default
  `/run/highland/control.sock`.
- Can you open it? A socket that is missing, or owned by another group, both
  look like "cannot connect" from the outside.
- Highland never listens on a network interface, by design. If you are trying to
  reach it from another machine, that is not supported and will not be.

## Commands do nothing

Check the [command reference](cli.md). Several commands are specified and
documented but not implemented in the current build, because the control socket
is not open yet. `version` and `check-config` are what work today; `run` loads
and validates the file, then exits 1. [`CHANGELOG.md`](../../CHANGELOG.md) records the rest.

## What to collect before asking for help

With these, a report is usually answerable without a round trip:

1. `highland status --json` from every node.
2. `highland events --limit 200` from every node, including the reasons.
3. The configuration, with secrets removed.
4. The version: `highland version`.
5. A packet capture from the segment: `sudo tcpdump -i <iface> -w highland.pcap 'ip proto 112'`.
6. The relevant kernel messages: `sudo dmesg | tail -100`.
7. For a suspected vulnerability, do not file a public issue; see
   [`SECURITY.md`](../../SECURITY.md).

## Next steps

- [Operations guide](operations.md) — the full runbook, including every
  transition reason
- [Health checks](health-checks.md) — writing checks that do not flap
- [Configuration reference](configuration.md) — every key and rule
