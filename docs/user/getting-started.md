# Getting Started

This walkthrough takes you from a fresh checkout to a running Highland
configuration. It takes about ten minutes and needs no root.

## What Highland does

Highland moves a virtual IP address (VIP) between machines so that a service
keeps a single address even when the machine holding it fails. It speaks VRRPv3,
the same protocol Keepalived uses, so it can share a segment with existing
implementations.

Two ideas are worth knowing before anything else.

**Ownership comes before advertisement.** A node only ever announces itself as
the owner of a VIP after the kernel has confirmed the address is really on its
interface. If it cannot take ownership, it says so and stays out. Highland has no
degraded mode in which a node claims a VIP it does not hold.

**Highland is a layer-2 tool.** It protects against a machine failing. It does
not protect against a network partition in which two machines can both reach
clients but not each other — in that case both can believe they own the address.
This is inherent to the protocol, not a Highland bug. Highland reports what it
sees and never claims agreement it does not have. See
[split-brain behavior](compatibility.md) before you deploy.

## Requirements

- Linux, on `x86_64` or `aarch64`
- A Rust toolchain (edition 2024; the minimum supported version is 1.85)
- For actual failover: `CAP_NET_ADMIN` and `CAP_NET_RAW`, normally granted
  through the systemd unit in `deploy/systemd/`

You do not need systemd. Highland is a single foreground process that also runs
under OpenRC or a bare container entrypoint.

## Build it

```console
$ git clone https://github.com/highland-rs/highland
$ cd highland
$ cargo build --workspace
$ cargo test --workspace
```

## Write a configuration

Highland reads one TOML file. Start from this one:

```toml
schema_version = 1

[node]
name = "node-a"

[logging]
level = "info"
format = "json"

[[instance]]
name = "api"
interface = "eth0"
vrid = 42
priority = 150
advertisement_interval = "1s"
preempt = true
preempt_delay = "30s"
startup_delay = "5s"

[instance.network]
mode = "unicast"
peers = ["192.0.2.12"]

[[instance.vip]]
address = "192.0.2.10/24"

[instance.health]
failure_policy = "weighted"
minimum_effective_priority = 100

[[instance.check]]
name = "api-ready"
type = "http"
url = "http://127.0.0.1:8080/ready"
expected_status = [200]
timeout = "500ms"
interval = "1s"
failure_threshold = 3
success_threshold = 2
weight = 100
```

Save it as `/etc/highland/config.toml`, owned by root and not writable by anyone
else. Highland refuses a world-writable configuration file unless you explicitly
tell it to accept one.

## Check it before you run it

```console
$ highland check-config /etc/highland/config.toml
/etc/highland/config.toml is valid: 1 instance(s), schema version 1
```

`check-config` reports every problem it finds at once, each labelled with the
rule that rejected it:

```text
highland: validating /etc/highland/config.toml: [V-01] instance.api.vrid: 0 is not a valid VRID; use 1..=255
```

Five mistakes account for most first attempts:

- **The two nodes disagree.** Both machines must use the same `vrid` and the same
  VIP. Otherwise they are two unrelated groups that will never see each other.
- **A peer is the node's own address.** Each node lists the *other* nodes, never
  itself.
- **A duration has no unit.** Write `500ms`, not `500`.
- **`priority` is 0.** Zero is reserved for the "I am giving up the address"
  signal and cannot be configured.
- **A check is missing the key its type needs.** An `http` check needs a `url` and
  an `expected_status`; a `tcp` check needs an `address`.

The [configuration reference](configuration.md) documents every key, every
default, and every rule.

## Set up the second node

Copy the file to the peer and change three things: `node.name`, `priority`, and
the peer list.

| Setting | node-a | node-b |
|---|---|---|
| `node.name` | `node-a` | `node-b` |
| `instance.vrid` | `42` | `42` (identical) |
| `instance.vip` | `192.0.2.10/24` | `192.0.2.10/24` (identical) |
| `instance.priority` | `150` | `100` |
| `instance.network.peers` | `["192.0.2.12"]` | `["192.0.2.11"]` |

The higher-priority node takes the address at startup. If you would rather the
lower-priority node keep it, set `preempt = false` on both.

## Run it

**Today this exits immediately**, with exit code 1:

```console
$ highland run --config /etc/highland/config.toml
ERROR highland: the VRRP transport is not implemented; the daemon will not start
```

The refusal is deliberate. Highland has no raw VRRP socket yet, and a process
that claims to be a VRRP router while sending nothing is worse than one that
declines to start. The configuration is still loaded and validated first, so a
broken file reports the broken file rather than this message.

Because of that, do not install the systemd unit yet. It has
`Restart=on-failure`, so it would restart the daemon every two seconds forever.
The unit becomes useful when the socket lands.

## Watch it

Highland exposes three ways to see what it is doing: structured logs, Prometheus
metrics, and an event stream. The event stream is the most useful when something
has gone wrong, because every role change arrives with the reason for it.

```console
$ highland status
$ highland status --json
$ highland show api
$ highland events --follow
```

None of these reach a running daemon yet, because there is no control socket.
The [operations guide](operations.md) is the runbook for when they do: what
each event means, what to do when the address will not move, and how to upgrade
without an outage.

## Changing the configuration later

Edit the file, check it, then reload:

```console
$ highland check-config /etc/highland/config.toml
$ highland reload
```

The reload applies the change in place where it can, and refuses the whole reload
where it cannot. `highland reload` and `SIGHUP` are the same operation, so an
operator who cannot send signals can still reload.

The design is all-or-nothing, and that is what you will get. If any part of a
reload cannot be applied, nothing changes and you get the reason; you are never
left half-way between two configurations. Changes that cannot be applied to a
live instance — its interface, its VRID, or its set of addresses — require
restarting that instance, and Highland reports which instances are affected
before it touches anything.

## Current state

Be aware of what works today. The state machine, the VRRPv3 codec, the
configuration layer, the netlink backend, the raw VRRP socket, the control
socket, the metrics endpoint, the event history, and the transactional reload are
all implemented and tested. A complete failover, including the announcement that
tells the segment the address moved, runs in the test suite against two real
network namespaces.

What is not done is IPv6 and multicast: the daemon speaks unicast IPv4, which is
what the test suite exercises. The IPv6 announcement is written and unit-tested
but has not been run against a real kernel, because there is nothing for it to
announce yet.

| You can do this now | Not yet |
|---|---|
| Build and test the project | Fail a VIP over between two real machines |
| Write and validate a configuration | Run with IPv6 virtual addresses |
| Encode and decode VRRP advertisements | Use multicast instead of unicast peers |
| Add and remove addresses, confirmed by read-back | Keepalived interoperability |
| Fail a VIP over between two nodes | Configure hold-down and retry parameters |
| Query a running node over the control socket | |
| Read the event history, and follow it | |
| Scrape metrics, reload without dropping the address | |

The [command reference](cli.md) marks every command with its state. Check
[`CHANGELOG.md`](../../CHANGELOG.md) for the current release notes.

## Next steps

- [Command reference](cli.md) — every command, and whether it works yet
- [Configuration reference](configuration.md) — every key and rule
- [Operations guide](operations.md) — the runbook
- [Compatibility](compatibility.md) — migrating from Keepalived
