# Highland

A memory-safe, observable, Linux-focused Rust implementation of high-availability
virtual IP failover, built around VRRPv3.

> **Status: Milestone 0.** The workspace, the protocol and domain types, the
> configuration layer with its full validation rule set, the observability
> primitives, and the control-API message model exist and are tested. VRRP
> traffic does not move a VIP yet; that arrives in Milestone 3. See
> [`docs/SPEC.md`](docs/SPEC.md) §27 for the milestone plan and
> [`CHANGELOG.md`](CHANGELOG.md) for what has landed.

## What Highland is

Highland is a declarative, explainable, testable failover controller for Linux
virtual IP ownership. It is a library first and a daemon second: the state
machine, the protocol codec, and the configuration validator are all usable
without the daemon.

- Correct VRRPv3 behavior for IPv4 and, from 1.0, IPv6
- Deterministic role transitions, every one with a machine-readable reason
- Native health checks instead of shell scripts
- Transactional configuration reload
- A control socket, a status API, and Prometheus metrics
- Split-brain behavior documented rather than papered over

## Scope

| Tier | Contents |
|---|---|
| `0.1.0` (Milestones 0–4) | Linux, IPv4, VRRPv3, unicast peers, one interface per instance, TCP and HTTP checks |
| `1.0` (Milestones 5–9) | IPv6, multicast, the full check set, Keepalived interoperability |
| Post-1.0 | BFD, cloud adapters, IPVS, privilege separation, simulation |

The full specification, including every requirement identifier, lives in
[`docs/SPEC.md`](docs/SPEC.md).

## Quick start

```console
$ cargo build --workspace
$ cargo test --workspace
$ cargo run -p highland-cli -- check-config crates/highland-config/tests/fixtures/basic.toml
basic.toml is valid: 1 instance(s), schema version 1
$ cargo run -p highland-cli -- --help
```

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

## Workspace layout

| Crate | Responsibility |
|---|---|
| `highland-core` | State machine, election, timers, health arithmetic. No Linux, no runtime |
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

- [Specification](docs/SPEC.md)
- [Architecture](docs/architecture.md)
- [Configuration](docs/configuration.md)
- [Operations](docs/operations.md)
- [Threat model](docs/threat-model.md)
- [Compatibility](docs/compatibility.md)
- [Testing](docs/testing.md)
- [Architecture decision records](docs/adr/)

## Privileges

The daemon needs `CAP_NET_ADMIN` and `CAP_NET_RAW`. It does not require
unrestricted root and does not modify firewall rules.

## License

Dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your
option.
