# Highland

A memory-safe, observable, Linux-focused Rust implementation of high-availability
virtual IP failover, built around VRRPv3.

> **Status: 0.2.0.** Milestones 0–6 have landed. A whole failover — election,
> ownership confirmed by kernel read-back, a takeover announced to the segment, and
> the address moving — runs in the test suite between two real network
> namespaces, over IPv4 and IPv6, unicast and multicast, with loss, reordering, a
> one-way partition, a link flap, and a frozen node injected into the segment.
> Keepalived interoperability is tested for IPv4 and IPv6, unicast and multicast.
> APIs are not stable before 1.0 ([`SPEC.md`](docs/SPEC.md) §26). See that
> §27 for the milestone plan and [`CHANGELOG.md`](CHANGELOG.md) for what landed.

## What Highland is

Highland is a declarative, explainable, testable failover controller for Linux
virtual IP ownership. It is a library first and a daemon second: the state
machine, the protocol codec, and the configuration validator are all usable
without the daemon.

- Correct VRRPv3 behavior for IPv4 and IPv6, unicast and multicast
- Deterministic role transitions, every one with a machine-readable reason
- Native health checks instead of shell scripts
- Transactional configuration reload
- A control socket, a status API, and Prometheus metrics
- Split-brain behavior documented rather than papered over

## Scope

| Tier | Contents |
|---|---|
| `0.2.0` (Milestones 0–6) | Linux, IPv4 and IPv6, VRRPv3, unicast and multicast peers, one family and one interface per instance, `tcp` / `http` / `unix` / `interface` checks |
| `1.0` (Milestones 7–9) | The operations interface, the full check set, Keepalived interoperability |
| Post-1.0 | BFD, cloud adapters, IPVS, privilege separation, simulation |

The full specification, including every requirement identifier, lives in
[`docs/SPEC.md`](docs/SPEC.md).

## Quick start

```console
$ cargo build --release --workspace
$ cargo test --workspace
$ cargo run -p highland-cli -- check-config crates/highland-config/tests/fixtures/basic.toml
basic.toml is valid: 1 instance(s), schema version 1
$ cargo run -p highland-cli -- --help
```

The release builds two binaries: `highland`, which operators type, and
`highland-daemon`, which `highland run` hands over to. Install both — see the
[installation guide](docs/user/installation.md).

A minimal configuration:

```toml
schema_version = 1

[node]
name = "node-a"

[[instance]]
name = "api"
interface = "eth0"
vrid = 42
priority = 150
advertisement_interval = "1s"

[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]

[[instance.vip]]
address = "192.0.2.10/24"

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

`check-config` parses the file, applies every validation rule, and reports each
violation with its specification rule and configuration key — it needs no
daemon and no privileges. The rest of the CLI talks to a running daemon over a
local Unix socket. See [Getting started](docs/user/getting-started.md) for the
walkthrough and [the CLI reference](docs/user/cli.md) for every command.

## What works, and what does not

| Area | State |
|---|---|
| Configuration model, strict TOML parser, `V-01`–`V-32` validation | Implemented, tested |
| `highland-core`: roles, events, actions, timers, election, health arithmetic, the instance state machine | Implemented, tested, no runtime dependency |
| `highland-vrrp`: encoding and decoding for IPv4 and IPv6, RFC 1071 checksums with the IPv6 pseudo-header, two-phase decoding | Implemented, tested, six fuzz targets in CI |
| `highland-net`: interface lookup, link state, address add and remove **confirmed by read-back**, multicast membership, gratuitous ARP and Neighbor Advertisements | Implemented, tested |
| The VRRP **socket**: sending, receiving, TTL 255, IPv4 and IPv6 | Implemented, tested against a real kernel |
| `highland-daemon`: executor, per-instance actor, interface monitor, health task, run loop, and the whole failover driven end to end between two namespaces | Implemented, tested |
| `highland-observe`: event model, redaction, bounded ring, Prometheus metrics | Implemented, tested |
| `highland-control`: message model, Unix-socket listener, rate limiter, error taxonomy | Implemented, tested over a real socket |
| `highland-checks`: spec, thresholds, debouncer, scheduler, and `tcp` / `http` / `unix` / `interface` probes | Implemented, tested. `https`, `dns`, `process`, `file`, and `composite` are refused by name |
| `highland-cli`: `run`, `status`, `show`, `events`, `reload`, `pause`, `resume`, `relinquish`, `force-transition`, `check-config` | Implemented, tested against a live daemon |
| Chaos: loss, one-way partition, reordering, duplication, link flap, frozen process | Implemented, five scenarios |
| Keepalived interoperability, IPv4 and IPv6, unicast and multicast | Implemented and tested; five of six scenarios converge, and it found a real protocol defect (the IPv4 checksum scope the RFC requires) |
| Keepalived as an IPv6 **unicast** master | Needs one configuration line: `allow_unconforming_hop_limit = true`. Keepalived sends a hop limit of 64 where §5.1.2.3 requires 255, and rejects `hop_limit` in its own configuration, so the peer cannot be corrected. Off by default, logged when used, and the cutover is in [`docs/user/migration.md`](docs/user/migration.md) |
| One instance holding both IPv4 and IPv6 addresses | Refused (`V-03`); use one instance per family |
| `sd_notify` readiness, configurable hold-down and retry, a `--yes` confirmation prompt | Not implemented |

`highland run` needs `CAP_NET_ADMIN` and `CAP_NET_RAW` and refuses to start
without them, rather than sit there claiming to be a VRRP router that cannot
move an address.
[`docs/user/troubleshooting.md`](docs/user/troubleshooting.md) has the rest.

The `README` never claims more than this table. If the table and the code
disagree, the table is the bug.

## Workspace layout

| Crate | Responsibility |
|---|---|
| `highland-core` | The state machine, election, timers, and health arithmetic. No Linux, no runtime |
| `highland-vrrp` | VRRPv3 types, encoding, decoding, validation |
| `highland-net` | Linux interface, address, and socket operations behind traits |
| `highland-checks` | Native health checks with thresholds and weights |
| `highland-config` | Typed configuration model, parser, and validation |
| `highland-observe` | Events, redaction, bounded history, sinks |
| `highland-control` | Control-API messages, errors, and rate limiting |
| `highland-daemon` | Process lifecycle, signals, shutdown |
| `highland-cli` | Administrative command-line interface |

Dependency rules and the rationale are in
[`docs/architecture.md`](docs/architecture.md).

## Documentation

Start at the [user documentation index](docs/user/index.md).

- [Specification](docs/SPEC.md)
- [Getting started](docs/user/getting-started.md)
- [Command reference](docs/user/cli.md)
- [Architecture](docs/architecture.md)
- [Configuration](docs/user/configuration.md)
- [Operations](docs/user/operations.md)
- [Threat model](docs/user/threat-model.md)
- [Compatibility](docs/user/compatibility.md)
- [Testing](docs/testing.md)
- [Architecture decision records](docs/adr/)

## Notices

- All code in the initial version was written by Space bunny alpha.

## Privileges

The daemon needs `CAP_NET_ADMIN` and `CAP_NET_RAW`. It does not require
unrestricted root and does not modify firewall rules.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your
option.
