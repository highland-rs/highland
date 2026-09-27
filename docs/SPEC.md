# Highland Specification

**Status:** Final
**Applies to:** Highland 0.1.0 and later, SemVer line `0.x` → `1.x`
**Supersedes:** `docs/SPEC_DRAFT.md`

---

## 0. How to Read This Document

### 0.1 Normative language

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHALL**, **SHOULD**, **SHOULD NOT**,
**MAY**, and **OPTIONAL** are to be interpreted as in RFC 2119 and RFC 8174, and only when
they appear in all capitals.

Where this document says "MUST", a test or a CI check MUST exist that fails when the
requirement is violated. See §21.

### 0.2 Requirement identifiers

Normative requirements carry a stable identifier so they can be referenced from tests,
pull requests, issues, and audits. Identifiers are never reused or renumbered.

| Prefix | Meaning |
|---|---|
| `G-nn` | Goal |
| `R-nn` | Requirement (product behavior) |
| `I-nn` | Invariant (must always hold) |
| `V-nn` | Configuration validation rule |
| `L-nn` | Hard resource limit |
| `S-nn` | Security requirement |
| `M-nn` | Milestone exit criterion |
| `D-nn` | Design principle |

Requirements without an explicit scope tag apply to every release.

### 0.3 Scope tags

Because Highland reaches several capabilities at different times, every requirement,
section, and configuration key is tagged with the earliest release that must provide it.

| Tag | Meaning |
|---|---|
| `[I]` | **Initial public release** (`0.1.0`, Milestones 0–4). |
| `[1]` | **Version 1.0** (Milestones 5–9). Everything in `[I]`, plus these. |
| `[F]` | **Post-1.0 / optional.** May be absent indefinitely. Absence MUST NOT break `1.0`. |

If a section is untagged, all of its normative statements are `[I]`.

### 0.4 Implementation status

This specification is the contract; the code is the implementation. They are kept
honest with respect to each other as follows:

- A requirement is **implemented** when a test enforces it. The test name or the
  test body carries the identifier.
- A requirement that is specified but not yet implemented is **planned**, and
  `CHANGELOG.md` records which milestone delivers it.
- Nothing in this document is weakened to match the code. When the code turns
  out to be wrong, the code changes. When the design turns out to be
  incomplete, this document changes and the change is recorded in Appendix A.

`docs/architecture.md` records the dependency edges as built, including edges
that differ from the target graph in §9.

### 0.5 Unresolved decisions

Decisions this document deliberately does **not** make are listed in Appendix B. They are
tracked as open questions, and no other part of this document may assume an answer to one.

---

## 1. Project Summary

**Highland** is a memory-safe, observable, Linux-focused Rust implementation of
high-availability virtual IP failover.

Its purpose is to be a modern alternative to the VRRP portion of Keepalived, with emphasis
on:

- Correct VRRPv3 behavior `[I]`
- Deterministic, explainable state transitions `[I]`
- IPv4 support `[I]`, IPv6 support `[1]`
- Unicast peer communication `[I]`, multicast peer communication `[1]`
- Native health checks instead of shell scripts `[I]`
- Split-brain resistance by honest disclosure rather than false claims `[I]`
- Structured observability `[I]`
- Strong configuration validation `[I]`
- Testability through network namespaces and simulation `[I]`
- A reusable Rust library, not only a daemon `[I]`

Highland is a Cargo workspace of focused crates, with a production daemon and CLI built on
top of them. See §7 and §8.

---

## 2. Product Positioning

Highland is not "VRRP written in Rust". Its value proposition is:

> A declarative, explainable, testable, and memory-safe Linux failover controller for
> virtual IP ownership.

It differs from the traditional shell-script-driven model by providing:

- Native checks for common cases `[I]`
- Explicit, inspectable role transitions with structured reasons `[I]`
- First-class metrics and an event history `[I]`
- Safer configuration semantics and transactional reload `[I]`
- A state machine that is testable without Linux networking `[I]`
- A library API usable by other Rust infrastructure projects `[I]`
- Both traditional multicast VRRP `[1]` and cloud-friendly unicast `[I]`

Highland is protocol-compatible with VRRP, but its internal API is NOT constrained by
historical configuration or implementation decisions.

---

## 3. Scope Tiers

This section is normative and resolves the scope questions that most often conflict.

### 3.1 `[I]` — Initial public release (0.1.0)

Exactly this set, and nothing wider:

- Linux, `x86_64` and `aarch64`
- VRRPv3, IPv4 only
- One VRRP instance bound to one interface, with one or more IPv4 VIPs
- Unicast peers only
- Priority election, preemption on/off, preemption delay
- Graceful shutdown with VIP relinquish
- TCP and HTTP health checks, weighted priority adjustment
- TOML configuration, JSON status output, Prometheus metrics
- Control socket with read-oriented commands plus `reload`, `pause`, `resume`,
  `relinquish`
- Network-namespace integration tests

### 3.2 `[1]` — Version 1.0

Adds:

- IPv6 VIPs, IPv6 advertisements, unsolicited Neighbor Advertisements
- Multicast peer mode for IPv4 and IPv6
- Mixed IPv4/IPv6 instances and mixed-family unicast peer lists
- HTTPS, DNS, Unix-socket, process, interface, file, and composite checks
- Keepalived protocol interoperability and a configuration importer
- Chaos testing, capability documentation, packaging

### 3.3 `[F]` — Post-1.0

- BFD
- Dynamic peer discovery
- Cloud floating-IP adapters
- IPVS integration
- Capability-based privilege separation via a helper process
- Simulation and replay mode as a shipped feature
- External fencing hooks
- A stable C ABI

### 3.4 Scope rules

- `R-nn` **MUST NOT** be implemented in a way that contradicts a lower scope tier. In
  particular, `[I]` code MUST NOT require IPv6, multicast, or any check type to function.
- A `[F]` feature MUST be behind an explicit, default-off Cargo feature flag, and the
  daemon MUST run correctly with every `[F]` feature disabled.
- This document is written from the `[1]` perspective. Where `[1]` and `[I]` differ, the
  `[1]` behavior is additive and MUST NOT change `[I]` behavior.

---

## 4. Goals

### 4.1 Primary goals `[I]`

Highland MUST:

- `G-01` Implement VRRPv3 for IPv4, and for IPv6 by `1.0`.
- `G-02` Support one or more independent VRRP instances.
- `G-03` Manage virtual IPv4 addresses on Linux interfaces, and IPv6 by `1.0`.
- `G-04` Support unicast peer mode, and multicast peer mode by `1.0`.
- `G-05` Perform deterministic `INIT`, `BACKUP`, and `MASTER` transitions.
- `G-06` Support priority-based master election with a documented tie-break.
- `G-07` Send and process advertisements correctly, including strict field validation.
- `G-08` Detect master failure within the protocol-defined timing budget (§13.3).
- `G-09` Add and remove VIPs safely, with ownership confirmed in the kernel.
- `G-10` Support native health checks that do not require a shell.
- `G-11` Expose structured logs and Prometheus metrics.
- `G-12` Provide a local administrative control interface.
- `G-13` Operate without shelling out for any core functionality.
- `G-14` Be testable in isolated network namespaces.
- `G-15` Provide a machine-readable reason for every role transition and failure.
- `G-16` Support graceful shutdown that relinquishes VIP ownership.
- `G-17` Remain usable in minimal Linux environments with no mandatory systemd dependency.

### 4.2 Secondary goals `[F]`

Highland should eventually:

- Import a useful subset of Keepalived configuration
- Support BFD
- Support dynamic peer discovery in controlled environments
- Support cloud-provider floating-IP adapters
- Support optional IPVS integration
- Support capability-based privilege separation
- Provide a simulation and replay mode
- Support embedded library use without the daemon
- Offer a stable C ABI if, and only if, there is demonstrated demand

---

## 5. Non-Goals

The first major release MUST NOT attempt to:

- Implement every Keepalived feature
- Implement IPVS load balancing
- Replace Pacemaker or Corosync
- Manage arbitrary routing protocols
- Implement a general-purpose service supervisor
- Execute arbitrary shell commands by default
- Support non-Linux operating systems
- Promise wire-level compatibility with every Keepalived-specific extension
- Modify firewall rules automatically
- Manage cloud provider APIs in the core crates
- Provide cluster membership or consensus beyond VRRP
- Guarantee protection against every possible network partition
- Hide the fundamental split-brain limitations of layer-2 failover
- Advertise `MASTER` without owning the VIPs, in any "degraded" mode (§11, `I-04`)

The product is a high-quality VRRP/VIP controller, not an infrastructure orchestration
platform.

---

## 6. Terminology

| Term | Meaning |
|---|---|
| Node | One running Highland daemon, identified by `[node].name` |
| VRRP instance | One logical VRRP group: VRID, priority, interface, VIP set, peer set |
| VRID | Virtual Router Identifier, 1–255 |
| Role | The state of an instance: `INIT`, `BACKUP`, `MASTER`, `FAULT`, or `DISABLED` |
| `INIT` | Startup state before the instance participates in election |
| `BACKUP` | Monitoring advertisements, eligible to become `MASTER` |
| `MASTER` | Owns the VIP set and sends advertisements |
| `FAULT` | A prior ownership attempt failed; ownership state is undefined; timers disarmed |
| `DISABLED` | Operator- or configuration-disabled; not participating; no timers armed |
| VIP | Virtual IP address managed by an instance |
| Advertisement | A VRRP packet announcing the current `MASTER` and its priority |
| `adver_int` | The advertisement interval, encoded in centiseconds on the wire |
| `Master_Down_Interval` | `3 × adver_int + Skew_Time`; the BACKUP takeover delay |
| `Skew_Time` | The local timer-resolution allowance, at most 0.01s |
| Address owner | The BackUp holding the highest-priority IP address on the segment, priority 255 |
| Configured priority | The `priority` value from configuration |
| Effective priority | Configured priority adjusted by health results (§12.2) |
| Eligibility | Whether the instance may hold or contest ownership (§12.1) |
| Preemption | A higher-priority `BACKUP` taking ownership from a lower-priority `MASTER` |
| Preemption delay | Configured wait before a higher-priority `BACKUP` preempts |
| Unicast peer | An explicit VRRP peer address used instead of multicast |
| Split brain | Two or more nodes simultaneously believing they are `MASTER` |
| Track item | A health or system condition that changes effective priority or eligibility |
| Check | A single health probe with thresholds, weight, and a result |
| Ownership | The kernel state in which a VIP is configured on an interface |
| Graceful relinquish | A deliberate `MASTER` → `BACKUP` transition that removes VIPs first |
| Generation | A monotonically increasing integer identifying a configuration revision |
| Control socket | The Unix domain socket exposing the local administrative API (§24) |
| Hold-down | A period during which an instance refuses to become `MASTER` after a fault |
| Elector | A check whose `weight` is greater than zero |
| Observational | A check whose `weight` is zero; recorded and exported, never affects election |

**Terminology disambiguation.** "Owner" in this document always means *address owner* as
defined above. The phrase "node owner" MUST NOT be used.

---

## 7. Deployment Model

### 7.1 Platform `[I]`

- Linux on `x86_64` and `aarch64`
- Network namespaces used by the test harness
- systemd-based and non-systemd environments

The daemon MUST NOT depend on systemd. systemd integration consists of unit files in
`deploy/systemd/` and optional readiness notification.

### 7.2 Privileges `[I]`

The daemon requires:

- `CAP_NET_ADMIN` to add and remove addresses and to inspect interfaces
- `CAP_NET_RAW` to open raw packet sockets in multicast mode and to emit gratuitous
  ARP / unsolicited Neighbor Advertisements
- Permission to read the configuration file
- Permission to create the control socket and, if enabled, the metrics listener

Privilege reduction after initialization is desirable, not required for `0.1.0`.
Capability documentation MUST be published with `1.0` (§27).

### 7.3 Operational environment `[I]`

- The daemon MUST NOT require a writable filesystem after startup, except for the
  configured state directory.
- The daemon MUST NOT modify firewall rules.
- The daemon MUST NOT install or depend on background services other than itself.

---

## 8. Workspace Structure

```text
highland/
├── Cargo.toml
├── Cargo.lock
├── LICENSE-APACHE
├── LICENSE-MIT
├── README.md
├── CONTRIBUTING.md
├── SECURITY.md
├── CHANGELOG.md
├── docs/
│   ├── SPEC.md
│   ├── architecture.md
│   ├── testing.md
│   ├── user/
│   │   ├── index.md
│   │   ├── getting-started.md
│   │   ├── installation.md
│   │   ├── configuration.md
│   │   ├── health-checks.md
│   │   ├── cli.md
│   │   ├── operations.md
│   │   ├── troubleshooting.md
│   │   ├── upgrading.md
│   │   ├── compatibility.md
│   │   └── threat-model.md
│   └── adr/
│       ├── ADR-0001-record-architecture-decisions.md
│       ├── ADR-0002-async-runtime.md
│       └── ADR-0003-netlink-library.md
├── crates/
│   ├── highland-core/
│   ├── highland-vrrp/
│   ├── highland-net/
│   ├── highland-checks/
│   ├── highland-config/
│   ├── highland-observe/
│   ├── highland-control/
│   ├── highland-daemon/
│   └── highland-cli/
├── tests/                     # shared fixtures, not compiled by cargo
│   ├── integration/
│   ├── network-ns/
│   ├── packet-captures/
│   └── compatibility/
├── fuzz/                      # reserved; populated in Milestone 2
│   ├── vrrp-packet/
│   ├── config-parser/
│   ├── check-response/
│   ├── netlink-event/
│   └── control-request/
└── deploy/
    ├── systemd/
    ├── openrc/
    └── containers/
```

Unit tests live in `#[cfg(test)] mod tests` inside the module they test, which
is the Rust convention and keeps private items testable.

- `R-01` Rust integration tests MUST live in `crates/<crate>/tests/`, next to the
  crate whose public API they exercise, because the workspace root is not a
  package. The workspace-level `tests/` directory holds fixtures shared by more
  than one crate: packet captures, recorded netlink transcripts, and namespace
  topologies. Tests that require root or network namespaces MUST be gated
  behind a Cargo feature so that `cargo test --workspace` succeeds
  unprivileged.
- `R-02` The workspace MUST define a single MSRV. A documented policy for bumping it MUST
  exist in `CONTRIBUTING.md`.

---

## 9. Crate Responsibilities

Each crate MUST depend only on crates listed for it or on crates strictly below it in this
list. Cycles are forbidden.

Two properties are load-bearing rather than stylistic. `highland-core` and
`highland-vrrp` MUST NOT depend on an async runtime, on Linux, or on each other,
because that is what makes the state machine and the codec testable with a fake
clock and no executor. And a crate MUST declare only the internal dependencies
it actually uses, so that an unused edge cannot hide a cycle or drag a
dependency into a consumer's build.

| # | Crate | May depend on | Runtime-aware |
|---|---|---|---|
| 1 | `highland-core` | none | no |
| 2 | `highland-vrrp` | none | no |
| 3 | `highland-config` | 1 | no |
| 4 | `highland-observe` | 1 | no |
| 5 | `highland-net` | 1, 2 | yes |
| 6 | `highland-checks` | 1, 4 | yes |
| 7 | `highland-control` | 1, 4 | yes |
| 8 | `highland-daemon` | 1–7 | yes |
| 9 | `highland-cli` | 3, 7 | yes |

Configuration does not depend on checks: checks are built from configuration, not
the other way round, which keeps the configuration layer reusable by importers
and by the control API.

### 9.1 `highland-core`

Pure domain logic. No Linux, no sockets, no filesystem, no process spawning, no direct
logging.

Responsibilities:

- Instance state machine and role transitions
- Election and tie-breaking
- Timer arithmetic
- Health-weight and effective-priority calculation
- Preemption logic
- Transition reasons and event types
- Clock and randomness abstractions
- Reload planning and generation tracking

`highland-core` MUST be usable in deterministic unit tests with a fake clock, and MUST NOT
reference a runtime, a wall clock, or a random source other than through its abstractions.

Suggested modules:

```rust
pub mod clock;
pub mod election;
pub mod health;
pub mod reload;
pub mod state;
pub mod timer;
pub mod transition;
pub mod types;
```

### 9.2 `highland-vrrp`

Protocol implementation, independent of sockets and of operating systems.

Responsibilities:

- VRRPv3 advertisement encoding and decoding
- Checksum calculation for both families
- Field, length, and address-count validation
- TTL / hop-limit and source-address validation helpers
- Version and packet-type recognition

`highland-vrrp` MUST NOT know about interfaces, peer configuration, or instance state. Peer
filtering (§14.3) is the caller's responsibility and MUST NOT be implemented here.

```rust
pub struct Advertisement {
    pub version: Version,
    pub vrid: Vrid,
    pub priority: Priority,
    pub advert_interval: Duration,
    pub addresses: Vec<IpAddr>,
}

pub fn encode_advertisement(
    advertisement: &Advertisement,
    family: IpFamily,
) -> Result<Vec<u8>, EncodeError>;

pub fn decode_advertisement(
    bytes: &[u8],
    family: IpFamily,
) -> Result<Advertisement, DecodeError>;
```

- `I-05` The decoder MUST NOT panic on any input, including arbitrary bytes, and MUST
  return an error instead. A fuzz target MUST assert this (§21.3).
- `I-06` `advertisement_interval` MUST round-trip exactly through the centisecond wire
  encoding, or the encoder MUST return an error if the duration is not representable.
- The decoder MUST reject, with a typed error: truncated packets, unsupported versions,
  VRID values of 0, priority values above 255, address counts of 0 or above 255, incorrect
  checksums, address families inconsistent with the requested family, malformed length
  fields, and unexpected packet types.

### 9.3 `highland-net`

Linux networking integration.

Responsibilities:

- Interface discovery, index resolution, up/down and carrier state
- Link-state monitoring
- Address addition and removal
- Gratuitous ARP and unsolicited Neighbor Advertisement `[I]` / `[1]`
- Raw and unicast packet sockets, multicast membership lifecycle `[I]` / `[1]`
- Netlink interaction
- Namespace-aware operations

```rust
pub trait NetworkBackend {
    fn interface(&self, name: &str) -> Result<Interface, NetError>;
    fn add_address(&self, interface: InterfaceId, address: IpCidr) -> Result<(), NetError>;
    fn remove_address(&self, interface: InterfaceId, address: IpCidr) -> Result<(), NetError>;
    fn send_gratuitous_update(&self, interface: InterfaceId, address: IpAddr)
        -> Result<(), NetError>;
}
```

- `R-03` All Linux-specific behavior MUST sit behind a trait, so that the daemon can be
  tested against a scripted backend.
- `R-04` The concrete Netlink implementation choice MUST be recorded in an ADR and MUST
  be covered by the `tests/network-ns/` suite (decision tracked in Appendix B, `B-02`).

### 9.4 `highland-checks`

Native health checking.

Check types: `tcp` `[I]`, `http` `[I]`, `https` `[1]`, `dns` `[1]`, `unix` `[1]`,
`process` `[1]`, `interface` `[1]`, `file` `[1]`, `composite` `[1]`.

```rust
pub struct CheckResult {
    pub status: CheckStatus,
    pub latency: Option<Duration>,
    pub reason: String,
    pub observed_at: Instant,
    pub sequence: u64,
}

pub enum CheckStatus {
    Passing,
    Failing,
    TimedOut,
    Disabled,
}
```

- `I-07` A check result MUST carry a monotonic `sequence` number. A result whose
  `sequence` is lower than the last accepted result for that check MUST be discarded
  (§11.4, invariant 13).
- `R-05` Check execution MUST be bounded by a configured timeout, a configured retry
  interval, and a global concurrency limit (§17.4). Checks MUST NOT run on the state
  machine's task.
- `R-06` Command execution (`type = "command"`) MUST be behind a default-off Cargo
  feature, MUST be explicitly enabled in configuration, and MUST restrict the executable
  to a configured allow-list of absolute paths. Its existence MUST be stated in the
  generated documentation as operationally unsafe.

### 9.5 `highland-config`

Configuration model, parser, validation, and redaction.

Format: TOML for human-authored input `[I]`; JSON for inspection output `[I]`; YAML is
`[F]` and is not planned.

The configuration MUST separate node-level settings, instance-level settings, network
settings, health policy, observability, and security/privilege settings.

Loading MUST support file parsing, schema validation, semantic validation, defaults,
source locations in errors, redacted display, and atomic reload preparation. Environment
variable substitution MAY be enabled explicitly `[F]`, and when enabled, substitution MUST
NOT apply to values marked non-substitutable.

- `I-08` Parsing MUST NOT panic. A malformed configuration MUST produce a diagnostic with a
  source location and MUST leave any running configuration untouched.

### 9.6 `highland-observe`

Logging, metrics, event stream, state snapshots, log redaction, transition explanations,
and optional OpenTelemetry `[F]`.

Surfaces `[I]`: logs to stderr or journald; a Prometheus metrics listener; a Unix-socket
status API; an optional JSON event stream.

- `S-01` Redaction MUST be applied to all log output, metrics labels, event payloads, and
  CLI output. A secret MUST NOT be observable in any of these surfaces.

### 9.7 `highland-control`

Local administrative API over a Unix domain socket with filesystem permission and group
ownership checks, and optional peer-credential verification.

- `S-02` The control API MUST NOT be reachable over a network transport in any release
  covered by this specification.
- `S-03` The control socket MUST be created with mode `0660` and, when `group` is
  configured, owned by that group. A socket with world-writable permissions MUST be
  rejected at startup with a fatal error.

The operation set is defined once, in §24.1. The CLI MUST map onto it exactly and MUST
NOT introduce operations absent from it.

### 9.8 `highland-daemon`

Production executable. Responsibilities: argument parsing, configuration loading,
logging and metrics initialization, component construction, health-check startup, protocol
listener startup, state-machine coordination, application of network ownership changes,
signal handling, graceful shutdown, and reload execution.

- `R-07` The daemon MUST use structured concurrency: every task has an explicit owner, an
  explicit cancellation path, and a documented shutdown ordering.
- `R-08` The state machine task MUST NOT perform blocking syscalls or await health checks.

### 9.9 `highland-cli`

Administrative CLI. It MUST connect to the control socket wherever an operation exists on
the daemon, and MUST NOT duplicate daemon logic or parse the configuration to answer
runtime questions.

---

## 10. Configuration Specification

### 10.1 Complete example (`0.1.0`, IPv4 unicast)

```toml
# /etc/highland/config.toml
schema_version = 1

[node]
name = "node-a"

[logging]
level = "info"
format = "json"

[metrics]
enabled = true
listen = "127.0.0.1:9900"

[control]
socket = "/run/highland/control.sock"
group = "highland"

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
peers = ["192.0.2.11", "192.0.2.12"]

[[instance.vip]]
address = "192.0.2.10/24"

[instance.health]
failure_policy = "weighted"
minimum_effective_priority = 100
all_checks_required = false

[[instance.check]]
name = "api-ready"
type = "http"
url = "http://127.0.0.1:8080/ready"
method = "GET"
timeout = "500ms"
interval = "1s"
failure_threshold = 3
success_threshold = 2
weight = 100
expected_status = [200]

[[instance.check]]
name = "database-port"
type = "tcp"
address = "127.0.0.1:5432"
timeout = "300ms"
interval = "2s"
failure_threshold = 2
weight = 50
```

### 10.2 IPv6 and multicast example (`1.0`)

```toml
[[instance]]
name = "api-v6"
interface = "eth0"
vrid = 43
priority = 120
advertisement_interval = "1s"
preempt = false
startup_delay = "5s"

[instance.network]
mode = "multicast"

[[instance.vip]]
address = "2001:db8:10::10/64"

[[instance.vip]]
address = "192.0.2.20/24"

[instance.network.multicast]
group = "ff02::12"     # default; per-family
ttl = 255             # must be 255
```

### 10.3 Key reference `[I]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `schema_version` | integer | — | Required. `1` is the only accepted value initially |
| `node.name` | string | hostname | Unique node name; used in all events |
| `logging.level` | enum | `info` | `error`, `warn`, `info`, `debug`, `trace` |
| `logging.format` | enum | `text` | `text`, `json` |
| `logging.redact` | array of string | `[]` | Configuration key paths whose values are redacted |
| `metrics.enabled` | bool | `false` | |
| `metrics.listen` | socket addr | none | Required if `enabled` |
| `control.socket` | path | `/run/highland/control.sock` | |
| `control.group` | string | none | Group ownership of the socket |
| `instance.name` | string | — | Required. Unique across the file |
| `instance.interface` | string | — | Required unless `defer_interface_binding` is enabled |
| `instance.vrid` | integer 1–255 | — | Required |
| `instance.priority` | integer 1–255 | 100 | 255 marks the address owner |
| `instance.advertisement_interval` | duration | `1s` | Bounds in `V-04` |
| `instance.preempt` | bool | `true` | |
| `instance.preempt_delay` | duration | `0s` | Forbidden when `preempt = false` |
| `instance.startup_delay` | duration | `0s` | Delay before entering election |
| `instance.defer_interface_binding` | bool | `false` | Skip existence checks at load time |
| `instance.network.mode` | enum | `multicast` | `unicast` in `[I]`; `multicast` requires `1.0` |
| `instance.network.peers` | array of IP | `[]` | Required in unicast mode; family inferred per entry |
| `instance.network.multicast.group` | IP | `224.0.0.18` / `ff02::12` | Per family |
| `instance.network.multicast.ttl` | integer | `255` | MUST be 255 |
| `instance.vip.address` | CIDR | — | At least one required; all must be one family `[I]` |
| `instance.health.failure_policy` | enum | `weighted` | `fail_closed`, `weighted`, `manual` |
| `instance.health.minimum_effective_priority` | integer 0–255 | `1` | Only with `weighted` |
| `instance.health.all_checks_required` | bool | `false` | Only with `fail_closed` |
| `instance.health.send_zero_priority_advert` | bool | `true` | Only with `fail_closed` |
| `instance.check.name` | string | — | Unique within the instance |
| `instance.check.type` | enum | — | Required |
| `instance.check.weight` | integer 0–254 | `0` | `0` makes the check observational |
| `instance.check.timeout` | duration | `1s` | |
| `instance.check.interval` | duration | `5s` | |
| `instance.check.failure_threshold` | integer ≥ 1 | `1` | Consecutive failures to enter failing |
| `instance.check.success_threshold` | integer ≥ 1 | `1` | Consecutive successes to recover |
| `instance.check.initial_grace_period` | duration | `0s` | Failures ignored during this window |
| `instance.check.retry_interval` | duration | `0s` | `0` means one attempt per interval |
| `instance.health.debounce` | duration | `0s` | Extra debounce before affecting election |

Removed keys from earlier drafts, with no alias retained because the format is
pre-release: `degrade` (merged into `weighted`), `instance_id` (unused), and
`CheckCriticality` (replaced by `weight` plus `all_checks_required`).

### 10.4 Validation rules

A configuration MUST be rejected when any of the following holds. Each rule has a
corresponding unit test and a fixture in `tests/integration/config/`.

| ID | Rule |
|---|---|
| `V-01` | `vrid` is outside 1–255 |
| `V-02` | `priority` is outside 1–255, or `priority = 0` is given |
| `V-03` | An instance contains VIPs of more than one address family while `1.0` multi-family support is unavailable (that is, in `0.x`) |
| `V-04` | `advertisement_interval` is below `10ms` or above `2550ms` |
| `V-05` | Two instances share the same `name` |
| `V-06` | Two instances share the same `(interface, vrid)` pair, regardless of family or mode |
| `V-07` | In unicast mode, a VIP family has no configured peer of that family |
| `V-08` | A configured peer is an address of this node |
| `V-09` | A configured peer is syntactically invalid or is a multicast address |
| `V-10` | `preempt_delay` is set while `preempt = false` |
| `V-11` | An instance has no VIP |
| `V-12` | The same IP address appears in more than one instance, or in one instance with two different prefix lengths |
| `V-13` | A VIP has a host prefix length of 0 or otherwise unusable mask |
| `V-14` | A check `timeout` is zero, or `interval` is zero, or `timeout > interval` |
| `V-15` | `failure_threshold` or `success_threshold` is 0 |
| `V-16` | `failure_policy = "fail_closed"` together with `minimum_effective_priority` |
| `V-17` | `failure_policy = "manual"` together with `minimum_effective_priority` or `all_checks_required` |
| `V-18` | `all_checks_required` or `send_zero_priority_advert` is set with a policy other than `fail_closed` |
| `V-19` | A check is electoral (non-zero `weight`) while the policy is `manual` |
| `V-20` | The sum of `weight` values in one instance is above 255 |
| `V-21` | `type = "command"` without the feature flag enabled, without `allow_paths`, or with a non-absolute or non-allow-listed path |
| `V-22` | A requested interface does not exist and `defer_interface_binding` is false |
| `V-23` | A check of type `http`/`https` has no `url`; `tcp` has no `address`; `expected_status` is empty for `http`/`https` |
| `V-24` | `multicast.ttl` is not 255 |
| `V-25` | Any per-instance hard limit in §17.4 is exceeded |
| `V-26` | The configuration file is world-writable, unless `--allow-insecure-config` is passed |
| `V-27` | The number of instances exceeds 256, the number of checks in one instance exceeds 64, or the file exceeds 4 MiB |
| `V-28` | `schema_version` is absent or unsupported |
| `V-29` | A referenced check name, metric, or control path duplicates another in a way that is not a valid scope (for example two checks with the same name in one instance) |
| `V-30` | `metrics.listen` is set while `metrics.enabled` is false, or is missing while enabled |
| `V-31` | The document configures no instances at all |
| `V-32` | `node.name` is absent or blank |

### 10.5 Reload semantics `[I]`

On reload the daemon MUST, in order:

1. Read the candidate file and parse it.
2. Validate syntax and schema.
3. Validate semantics, including the whole-file rules above.
4. Allocate a new `Generation`, greater than the active one.
5. Compute a per-instance change plan and classify each instance as `Unchanged`,
   `Reloadable`, or `RestartRequired` (§10.6).
6. Log or render the plan, including every change and every rejection.
7. Apply `Reloadable` instances' changes; create, pause, or retire instances accordingly.
8. Reject the entire reload if any instance is `RestartRequired` and
   `allow_restart_required` is false.

- `I-09` A rejected reload MUST leave the active configuration, active generation, and all
  instance state byte-for-byte unchanged.
- `I-10` Reload MUST NOT interrupt an instance classified `Unchanged`. Its timers,
  ownership, and sequence numbers continue uninterrupted.
- `I-11` The previous accepted configuration MUST be retained in memory until the next
  successful reload, so that a rejected reload is always revertible and the running
  configuration can always be dumped for diagnosis.
- `I-12` Every result that carries a generation MUST be discarded if the generation is
  older than the instance's current generation. This includes health results, timers, and
  network-operation results.

### 10.6 Change classification

| Change | Classification |
|---|---|
| `priority`, `preempt`, `preempt_delay`, `startup_delay`, `health`, `check` | `Reloadable` |
| `advertisement_interval` | `Reloadable` when the instance is not `MASTER`; `RestartRequired` when it is `MASTER` |
| `peers` | `Reloadable` |
| `interface`, `vrid` | `RestartRequired` |
| Adding or removing a VIP | `RestartRequired` |
| Changing `network.mode` | `RestartRequired` |
| Removing an instance | `RestartRequired`; the instance relinquishes its VIPs before the task ends |

- `I-13` When a `MASTER` instance must change its VIP set, it MUST first stop advertising,
  then remove the departed VIPs, then add the new VIPs, then resume advertising. The
  ordering MUST be observable in the event stream.

---

## 11. VRRP State Machine

The state machine is synchronous, deterministic, and free of I/O. It is the only
authority over role.

- `I-01` An instance has exactly one role at every instant, including before the first event
  and after teardown. A new instance starts in `INIT`.
- `I-02` A role change is atomic from any observer's point of view. No intermediate role is
  ever exposed.
- `I-03` Ownership is defined only in `MASTER`. An instance "owns" a VIP when that address
  is present on its interface in the kernel and has been confirmed by read-back.
- `I-04` A node MUST NOT transmit an advertisement announcing `MASTER` unless it is in
  `MASTER` role and has confirmed ownership of every configured VIP. There is no
  degraded-advertisement exception in any scope tier (§5).

### 11.1 Roles

```rust
pub enum Role {
    Init,
    Backup,
    Master,
    Fault,
    Disabled,
}
```

| Role | Owns VIPs | Sends advertisements | Advertises priority | Timers |
|---|---|---|---|---|
| `INIT` | no | no | — | startup delay only |
| `BACKUP` | no | no | — | `Master_Down_Interval`, preemption delay |
| `MASTER` | yes | yes | effective priority | advertisement timer |
| `FAULT` | no | no | — | hold-down, retry backoff |
| `DISABLED` | no | no | — | none |

- `I-14` A role other than `MASTER` MUST NOT own VIPs, and `MASTER` MUST own every
  configured VIP (§5 definitions of the roles make this symmetric).
- `I-15` A node MUST NOT enter `MASTER` while its interface is down, missing, or has no
  carrier.
- `I-16` `FAULT` MUST NOT claim ownership and MUST NOT send advertisements, including the
  zero-priority relinquish advertisement.

### 11.2 Events

```rust
pub enum Event {
    Startup,
    StartupDelayElapsed,
    InterfaceUp,
    InterfaceDown,
    CarrierLost,
    AdvertisementReceived(PeerAdvertisement),
    AdvertisementTimeout,
    HealthChanged(HealthSummary),
    TimerExpired(TimerId),
    ActionFailed { kind: ActionKind, error: ActionError },
    OperatorPauseRequested,
    OperatorResumeRequested,
    OperatorRelinquishRequested,
    OperatorForceTransitionRequested { target: Role, reason: String },
    ConfigurationReloaded { generation: Generation, plan: InstancePlan },
    ShutdownRequested,
}
```

The set is closed apart from the `#[non_exhaustive]` marker, which exists so that
adding an event is a compatible change for downstream matchers.

A `PeerAdvertisement` is the domain-level view of a decoded advertisement:
VRID, priority, source address, and the interval the peer claims. The state
machine never sees a wire type, which is what keeps `highland-core` independent of
`highland-vrrp` (§9.2).

Success is reported, not assumed. A machine that was told only about failures
would have to enter `MASTER` optimistically, which is precisely the shape that
lets a node advertise an address it does not hold. Reporting both outcomes makes
`I-04` structural: there is no path from `BACKUP` to `MASTER` and to an
advertisement except through `ActionSucceeded { kind: AddAddresses }`.

- `R-09` Every `Operator*` event MUST originate from an authenticated control request, MUST
  be recorded in the audit log, and MUST carry the requesting peer credential.
- `R-10` `OperatorForceTransitionRequested` MUST be rejected unless the daemon was started
  with `--enable-force-transition`, and MUST be recorded in the audit log when accepted.

### 11.3 Actions

```rust
pub enum Action {
    ArmTimer { timer: TimerId, deadline: Duration },
    CancelTimer { timer: TimerId },
    SendAdvertisement { priority: u8 },
    AddVirtualAddresses,
    RemoveVirtualAddresses,
    SendGratuitousUpdates,
    SetEffectivePriority { priority: u8 },
    EnterRole { role: Role, reason: TransitionReason },
    EmitEvent { name: &'static str },
    Log { level: LogLevel, message: String },
}
```

Timers are armed with an **absolute deadline** in the clock's own units, not
with a relative delay. A relative delay would make the state machine's output
depend on when the executor happened to apply the previous action, which would
break determinism (`R-27`). Hold-down and retry are timers like any other, so
there is no separate `SetHoldDown` or `ScheduleRetry` action; `ActionKind` is the
coarse classification used when a failure is reported back, and it is deliberately
coarser than `Action` so that adding an action does not require a new failure
path.

- `I-17` The state machine MUST NOT perform I/O. All side effects MUST be expressed as
  actions.
- `R-11` Every action the executor fails MUST be reported back as
  `Event::ActionFailed` with the `ActionKind`, so the machine can decide whether to retry,
  enter `FAULT`, or remain in place.
- `I-18` `SendAdvertisement` MUST be discarded, with a warning, if the executor reports
  that VIP ownership is not currently held. Advertising without ownership is forbidden
  (§5, `I-04`).

### 11.4 Executor contract

| Action | Success | Failure behavior |
|---|---|---|
| `ArmTimer` | Timer registered for the instance | Internal error; instance enters `FAULT` |
| `SendAdvertisement` | Packet written | Counted, logged; after 3 consecutive failures in 10s, `FAULT` |
| `AddVirtualAddresses` | Addresses confirmed present in the kernel | `FAULT`, hold-down, bounded retry (§11.5) |
| `RemoveVirtualAddresses` | Addresses confirmed absent | The role stays `MASTER` with advertising stopped, because the addresses are still present and `I-14` forbids a non-master role from owning them. Removal is retried with backoff, and the role changes only once removal is confirmed |
| `SendGratuitousUpdates` | Packets written | Logged; ownership is not affected |
| `SetEffectivePriority` | Priority recorded | Internal error |

- `I-19` Confirmation means reading the address back from the kernel, not merely observing
  a successful netlink acknowledgment.

### 11.5 Retry and hold-down `[I]`

- `R-12` Retries MUST use bounded exponential backoff with a configurable base of 1s and a
  cap of 30s, with a documented maximum attempt count of 10. After the cap, the instance
  remains in `FAULT`, emits an actionable event, and stops retrying until the operator
  intervenes or a new generation is loaded.
- `I-20` Entering `FAULT` MUST first attempt to remove any partially added VIPs. If removal
  fails, the event MUST record which addresses could not be removed.
- `I-21` A node in `FAULT` MUST NOT transition to `MASTER` without an operator resume, a
  configuration reload, or the hold-down expiring *and* a successful re-verification of
  interface availability and address absence.

---

## 12. Election and Priority Model

### 12.1 Eligibility

An instance is eligible for ownership when all of the following hold:

- Role is not `DISABLED`
- The interface exists, is administratively up, and has carrier
- `startup_delay` has elapsed
- Health policy permits (§12.2)

- `I-22` A node MUST NOT become `MASTER` while ineligible, and a `MASTER` that becomes
  ineligible MUST leave `MASTER` according to §12.4.

### 12.2 Health policies

Exactly three policies exist. `degrade` from earlier drafts is merged into `weighted`; the
configuration has no alias.

| Policy | Effect of a failing check |
|---|---|
| `manual` | None. Results are recorded, exported, and emitted as events only. |
| `weighted` | Reduces effective priority by the check's `weight`. |
| `fail_closed` | Makes the instance ineligible, causing immediate relinquish. |

**`weighted`** arithmetic:

```text
penalty      = sum of weight over checks currently in Failing state
raw          = configured_priority - penalty
effective    = max(minimum_effective_priority, raw)
eligible     = effective > 0
```

- `I-23` `effective` MUST be at least 1 while the node is eligible. A computed `effective`
  of 0 means "must not be `MASTER`" and the node MUST NOT transmit a priority of 0; it
  relinquishes silently and stays silent.
- `I-24` `effective` MUST never exceed `configured_priority`.

**`fail_closed`** arithmetic:

```text
blocking_checks = if all_checks_required { all checks }
                  else { checks with weight > 0 }
eligible        = no check in blocking_checks is in Failing state
```

- `I-25` A `fail_closed` instance MUST NOT become `MASTER` while any blocking check is
  failing, and a `MASTER` in that condition MUST relinquish immediately (§12.4).

**Both policies:**

- `R-13` Health evaluation MUST be debounced by `health.debounce` and by the per-check
  `failure_threshold` and `success_threshold`. A single failed probe MUST NOT change
  election state.
- `R-14` `weight` semantics: a check with `weight = 0` is observational. It is exported
  and emitted, but never contributes to `penalty` and never triggers `fail_closed`, even
  when `all_checks_required = true`.
- `I-26` A health result whose `sequence` is older than the last accepted result for that
  check MUST be discarded, so a slow check can never overwrite a newer verdict.

### 12.3 Tie-breaking `[I]`

Applied in order, deterministically, when effective priorities are equal:

1. Higher effective priority wins.
2. If equal, the higher **primary IP address** of the instance's interface wins. Within an
   interface, the primary address of the family of the advertisement is compared. When an
   instance is configured with both families, IPv4 is compared first, then IPv6.
3. If still equal, the incumbent `MASTER` keeps the role; a starting or returning node
   yields and remains `BACKUP`. This makes simultaneous startup converge on exactly one
   node without a tie-break timer.
4. If there is no incumbent, the node with the lexicographically smaller peer-address set
   wins. This case is only reachable on a segment where two nodes report the same address,
   and it MUST be logged as `ambiguous_tie_break`.

- `R-15` The tie-break MUST be documented in `docs/architecture.md` and MUST be covered by
  property tests asserting that exactly one node is selected for any input pair.
- `R-16` A `MASTER` that receives a valid advertisement from a peer whose effective
  priority is strictly greater than its own MUST transition to `BACKUP`, with reason
  `higher_priority_peer_advertisement`, regardless of the `preempt` setting. `preempt`
  governs only whether a `BACKUP` may take over an existing `MASTER`.

### 12.4 Relinquishment

| Trigger | Behavior |
|---|---|
| Health `fail_closed` and a blocking check is failing | Send a zero-priority advertisement (if `send_zero_priority_advert`), stop advertising, remove VIPs, enter `BACKUP` |
| Health `weighted` and a higher-priority peer advertises | Step down per `R-16` |
| Operator relinquish | Zero-priority advertisement, remove VIPs, enter `BACKUP` |
| Shutdown | Zero-priority advertisement, remove VIPs, then exit (§14.5) |
| Interface down or carrier lost | Advertisement task stops, VIPs removed if the kernel permits, enter `FAULT` |
| `preempt = false` and a `MASTER` is running | No change; a `BACKUP` never preempts |

- `I-27` Zero-priority advertisements are sent only on the paths above. A node MUST NOT
  send a priority-0 advertisement in any other circumstance.

### 12.5 Preemption `[I]`

Taking ownership after the preemption delay expires carries the transition reason
`preemption_delay_elapsed`.

- A `BACKUP` with `preempt = true` that receives an advertisement with a strictly lower
  effective priority starts the preemption-delay timer.
- The preemption-delay timer is NOT restarted by further lower-or-equal priority
  advertisements. It IS cancelled by an advertisement with equal or greater effective
  priority, by becoming ineligible, by the interface going down, by an operator pause, and
  by shutdown.
- With `preempt_delay = 0s`, takeover happens on the next advertisement processing tick.
- With `preempt = false`, a `BACKUP` takes over only after `Master_Down_Interval` expires,
  and it then uses its own configured priority.
- A returning higher-priority node MUST complete `startup_delay` before entering election,
  and MUST NOT preempt before its health checks have had at least one full evaluation
  window.

- `I-28` Oscillation caused by short-lived health failure MUST be prevented by
  `failure_threshold` plus `health.debounce`. It MUST NOT be prevented by an undocumented
  heuristic.

---

## 13. Timers

Every timer has a single owning instance, a documented duration function, a cancellation
path, and an ID in `TimerId`.

| Timer | Armed when | Duration | Cancelled by |
|---|---|---|---|
| Startup delay | `Startup`, `InterfaceUp`, resume | `startup_delay` | Shutdown, pause |
| Advertisement | Entering `MASTER` | `advertisement_interval` | Leaving `MASTER`, pause, interface down |
| Master-down | Entering `BACKUP` | `3 × adver_int + Skew_Time` | Receiving a valid advertisement, leaving `BACKUP` |
| Preemption delay | Seeing a lower-priority advert, `preempt = true` | `preempt_delay` | See §12.5 |
| Hold-down | Entering `FAULT` | `hold_down`, default 10s | Operator action, reload |
| Retry | Failed action | backoff, base 1s, cap 30s | Successful action, reload |

Health-check intervals are not core timers. A check owns its own schedule and
delivers results as `Event::HealthChanged`; the state machine never schedules a
probe, which is what keeps `highland-core` free of check knowledge (`I-38`).

- `I-29` Every timer MUST be cancellable, and cancellation MUST be idempotent.
- `I-30` Re-delivery of an expired timer MUST NOT occur after cancellation, including
  across a configuration reload.

### 13.3 Timing budget `[I]`

With `advertisement_interval = 1s`:

| Event | Budget |
|---|---|
| Backup takes over after master silence | ≤ 3.06s from the last advertisement |
| Master advertises after a health failure crosses threshold | ≤ `interval` + `timeout` + `debounce` |
| Master takes over from a failed peer under `fail_closed` | ≤ 1.06s after the zero-priority advertisement |
| Graceful shutdown to VIP removal | ≤ 5s default (`shutdown.budget`) |

These numbers are derived from RFC 5798 §6.1 and MUST be asserted in the netns test suite.

---

## 14. Networking Requirements

### 14.1 Becoming `MASTER`

1. Verify the interface exists, is up, and has carrier.
2. Verify each VIP is not already present on any local interface, unless it is one this
   instance already owns.
3. Add the VIPs.
4. Read the addresses back to confirm ownership (`I-19`).
5. Send gratuitous ARP for each IPv4 VIP.
6. Send unsolicited Neighbor Advertisements for each IPv6 VIP.
7. Arm the advertisement timer and send the first advertisement.
8. Emit a `role_transition` event with the resulting effective priority.

If a critical step fails, the node MUST NOT claim `MASTER`; it enters `FAULT` per §11.5
and emits an actionable error naming the interface, address, and kernel error.

### 14.2 Becoming `BACKUP`

1. Stop advertising.
2. Remove the VIPs.
3. Read the addresses back to confirm removal.
4. Arm the master-down timer.
5. Emit a relinquish event.

### 14.3 Peer handling `[I]` unicast, `[1]` multicast

- Peer lists are explicit in unicast mode, and per family.
- Advertisements MUST be validated before any state machine delivery: source address MUST
  be a configured peer, the packet MUST arrive on the bound interface, TTL or hop limit
  MUST be 255, the VRID MUST match the instance, and the version MUST be acceptable.
- Rejected packets MUST be counted by reason and MUST NOT reach the state machine.
- Duplicate suppression: a repeated advertisement identical to the currently processed one
  MUST still reset the master-down timer, and MUST NOT be counted twice in
  `advertisements_received_total`. It SHOULD be recorded as a duplicate.
- Peer reachability MUST be reported per peer. A peer that becomes unreachable MUST NOT
  by itself change the local role, except that a peer set of size 0 MUST place the
  instance in `INIT`.
- In multicast mode the group MUST be `224.0.0.18` for IPv4 and `ff02::12` for IPv6 unless
  overridden; TTL MUST be 255; membership MUST be joined on entering the instance and left
  on teardown.

### 14.4 Network namespaces `[I]`

The test harness MUST be able to build:

```text
node-a namespace ─ veth ─ bridge ─ veth ─ node-b namespace
```

and MUST be able to inject: link failure, packet loss, delay, reordering, duplication,
partition, interface restart, address-add and address-remove failure, address conflict,
concurrent startup, and asymmetric reachability.

### 14.5 Shutdown `[I]`

On `SIGTERM` or `SIGINT`, in order and within `shutdown.budget` (default 5s):

1. Stop accepting new control requests.
2. Stop health checks.
3. For each `MASTER`: send a zero-priority advertisement, then remove VIPs, then confirm
   removal.
4. Emit a final state dump.
5. Exit `0`.

If the budget expires, the daemon removes what it can, emits an event naming the
addresses it could not remove, and exits non-zero.

- `I-31` Shutdown MUST be idempotent: a second signal MUST NOT start a second sequence.
- `I-32` After shutdown begins, the daemon MUST NOT add a VIP.

---

## 15. Health Checks

### 15.1 Check types

`tcp` `[I]`, `http` `[I]`: connect, send, read headers, validate `expected_status`, bound
the response body, honor the timeout.

`https` `[1]`: certificate validation enabled by default; configurable trust roots; SNI
required; explicit `insecure = true` opt-in which MUST emit a warning event on every
transition to failing-for-certificate; response-size limit.

`dns` `[1]`: `A`, `AAAA`, `SRV`, `TXT`; resolver address configurable; a successful query
with zero answers MUST be a failure.

`unix` `[1]`: connect to a path; the path MUST be confined to an allowed base directory
(§17.2).

`process` `[1]`: existence only. It is observational by default and MUST be documented as
a weak signal that does not imply readiness.

`interface` `[1]`: link present, link up, carrier, optional address presence.

`file` `[1]`: existence, with a canonicalized path confined to an allowed base directory.

`composite` `[1]`: boolean or weighted aggregation of other checks.

`command` `[F]`: see `R-06`.

### 15.2 Per-check configuration `[I]`

`name`, `type`, `interval`, `timeout`, `failure_threshold`, `success_threshold`, `weight`,
`initial_grace_period`, `retry_interval`. Per-check resource limits are global (§17.4).

### 15.3 Hysteresis `[I]`

A check occupies one of two stable states, `Passing` and `Failing`, plus `Disabled` when
the check is not running. It enters `Failing` after `failure_threshold` consecutive
failures, returns to `Passing` after `success_threshold` consecutive successes, and
ignores all results during `initial_grace_period`. `TimedOut` counts as a failure; it is
not a stable state.

The event stream MUST distinguish, as separate event types: single failed probe, check
entered failing, check recovered, instance became ineligible, instance became eligible.

### 15.4 Execution model `[I]`

Each check runs on a bounded task with an explicit concurrency limit. Results are
delivered to the instance's health channel with a `sequence` number and the active
`Generation`. A result is discarded if either is stale (`I-12`, `I-26`). Checks never
block the state machine task (`R-05`, `I-38`).

---

## 16. Observability

### 16.1 Structured events `[I]`

Every role transition, health state change, ownership change, configuration
reload, and operator action MUST produce an event. The event model has a fixed
shape so that consumers can rely on it:

| Field | Type | Notes |
|---|---|---|
| `name` | closed enum | The event name (`R-17`) |
| `level` | `info`, `warn`, `error` | |
| `node` | string | Node name |
| `instance` | string or null | Null for node-wide events |
| `from`, `to` | string or null | Role before and after, when the event describes a transition |
| `reason` | string | The machine-readable reason (§16.1.1) |
| `fields` | array of key and value pairs | Variable detail: peer, priorities, timer durations |
| `timestamp` | RFC 3339 UTC | |

```json
{
  "name": "role_transition",
  "level": "info",
  "node": "node-a",
  "instance": "api",
  "from": "MASTER",
  "to": "BACKUP",
  "reason": "higher_priority_peer_advertisement",
  "fields": [
    { "key": "peer", "value": "192.0.2.11" },
    { "key": "local_priority", "value": 120 },
    { "key": "remote_priority", "value": 150 }
  ],
  "timestamp": "2027-01-03T12:00:14.123Z"
}
```

Field values are a closed set of scalar types (boolean, integer, float, text), so
that a consumer never has to guess how to render one. Arbitrary detail belongs in
`fields` rather than in new top-level keys, which is what lets the shape stay
stable as events gain detail.

#### 16.1.1 Transition reasons

`reason` is a closed enum, because operators and dashboards depend on its
spelling. The set is `startup`, `interface_up`, `interface_down`,
`master_down_timeout`, `higher_priority_peer_advertisement`,
`preemption_delay_elapsed`, `health_ineligible`, `operator_relinquish`,
`operator_force_transition`, `configuration_reloaded`, `hold_down_expired`,
`ownership_failed`, and `shutdown`. Each one is documented in
`docs/user/operations.md` with what an operator should check when it appears.

- `R-17` The set of `reason` values is a closed, documented enum. Any new reason MUST be
  added to `docs/user/operations.md` in the same change.
- `R-18` The event stream MUST be replayable from an in-memory ring buffer of at least 4096
  entries (`L-08`) and MUST be streamable as JSON lines.

### 16.2 Metrics `[I]`

| Metric | Type | Labels |
|---|---|---|
| `highland_build_info` | gauge, always 1 | `version` |
| `highland_up` | gauge | — |
| `highland_instance_role` | gauge | `instance` |
| `highland_instance_effective_priority` | gauge | `instance` |
| `highland_instance_health` | gauge | `instance` |
| `highland_instance_vips_owned` | gauge | `instance` |
| `highland_instance_transitions_total` | counter | `instance`, `from`, `to` |
| `highland_advertisements_sent_total` | counter | `instance`, `family` |
| `highland_advertisements_received_total` | counter | `instance`, `family` |
| `highland_rejected_packets_total` | counter | `instance`, `reason` |
| `highland_master_down_events_total` | counter | `instance` |
| `highland_vip_add_failures_total` | counter | `instance` |
| `highland_vip_remove_failures_total` | counter | `instance` |
| `highland_check_failures_total` | counter | `instance`, `check` |
| `highland_check_duration_seconds` | histogram | `instance`, `check` |
| `highland_reloads_total` | counter | `result` |
| `highland_control_requests_total` | counter | `command`, `result` |

- `R-19` `highland_instance_role` encoding MUST be documented: `0 = INIT`, `1 = BACKUP`,
  `2 = MASTER`, `3 = FAULT`, `4 = DISABLED`.
- `R-20` Labels MUST NOT contain peer addresses, addresses, or error strings. Peer-level
  data belongs in the event stream. The total label cardinality MUST be bounded by
  construction (`L-10`).

### 16.3 Status API `[I]`

The status response is the control API's `NodeStatus` message, serialized as
JSON. It is defined in `highland-control` so that the CLI and the daemon cannot
disagree about its shape.

```json
{
  "node": "node-a",
  "generation": 7,
  "uptime_seconds": 412.5,
  "instances": [
    {
      "name": "api",
      "role": "MASTER",
      "priority": 150,
      "effective_priority": 100,
      "vip_addresses": ["192.0.2.10/24"],
      "vips_owned": true,
      "health": "degraded",
      "master_down_remaining_ms": null,
      "preemption_remaining_ms": null,
      "last_reason": "health_threshold_exceeded"
    }
  ]
}
```

The response carries the configured and effective priority, whether the VIPs are
present in the kernel, the health verdict, the two timer remainders, and the reason
for the most recent transition. The active `Generation` is included so that a
caller can tell whether the status it is reading describes the configuration it
asked about (`I-12`).

Per-check detail and per-peer reachability are returned by the `events` and
`instances` operations respectively rather than being embedded in every status
response, so that the common case stays small.

- `R-21` `master_down_remaining_ms` and `preemption_remaining_ms` MUST be `null` when the
  corresponding timer is not armed.
- `R-22` The status API MUST be a pure read. It MUST NOT mutate instance state.

---

## 17. Security Requirements

### 17.1 Untrusted input `[I]`

All network input is untrusted.

- `S-04` No parser MUST panic on untrusted input.
- `S-05` Packet size, address count, event queue depth, HTTP response body, DNS reply
  size, control request size, and command output MUST all be bounded.
- `S-06` Every network operation MUST have a timeout.
- `S-07` Configuration files and packets MUST NOT cause unbounded allocation.

### 17.2 Configuration and secrets `[I]`

- Refuse world-writable configuration files unless explicitly overridden (`V-26`).
- Never log secrets; redaction is applied at the observability layer (`S-01`).
- Never accept secrets through process arguments.
- Validate control-socket permissions at startup.
- Command execution is opt-in and path-restricted (`R-06`).
- File-based checks MUST confine paths to an allowed base directory.

### 17.3 Privilege `[I]`

Document required capabilities. The future privilege-separation model is `[F]` and MUST
NOT be implied by this release: the daemon runs with `CAP_NET_ADMIN` and `CAP_NET_RAW`, and
MUST NOT require a helper process.

### 17.4 Hard limits `[I]`

| ID | Limit | Value |
|---|---|---|
| `L-01` | Addresses in one advertisement | 255 |
| `L-02` | Configured VIPs per instance | 255 |
| `L-03` | Unicast peers per instance | 255 |
| `L-04` | Instances per daemon | 256 |
| `L-05` | Checks per instance | 64 |
| `L-06` | Configuration file size | 4 MiB |
| `L-07` | Concurrent health checks | 64 across the daemon |
| `L-08` | Event ring buffer | 4096 entries |
| `L-09` | HTTP/HTTPS/DNS response body | 64 KiB |
| `L-10` | Metric label cardinality | bounded by construction, no dynamic values |
| `L-11` | Advertisement packet acceptance rate | 2000/s per instance, above which excess is dropped and counted |
| `L-12` | Control request rate | 20/s per peer credential |
| `L-13` | Reload rate | 1 per 5s, excess requests are rejected with an audit event |
| `L-14` | Control request size | 64 KiB |
| `L-15` | Command output capture | 64 KiB |

Exceeding a limit MUST produce a typed error or a counter increment, never unbounded growth
and never a crash.

---

## 18. Error Model

Errors are typed, contextual, and actionable, using `thiserror` in libraries and `anyhow`
only in the daemon and CLI binaries.

**Each crate owns exactly one public error type.** A single crate-wide aggregate
was considered and rejected: the CLI depends only on `highland-config` and
`highland-control` (§9), so an aggregate that mentions network and check errors
would force the CLI to depend on `highland-net` and `highland-checks` for no
benefit, and would make the dependency graph of §9 unenforceable.

| Crate | Error type | Domain |
|---|---|---|
| `highland-core` | `CoreError` | Timer arithmetic and domain invariants |
| `highland-vrrp` | `ProtocolError`, `EncodeError`, `DecodeError`, `AdvertisementError` | Wire values |
| `highland-net` | `NetError` | Interfaces, addresses, sockets |
| `highland-checks` | `CheckError` | Check configuration and results |
| `highland-config` | `ConfigError` | Reading, parsing, and validating configuration |
| `highland-control` | `ControlError` | Control requests and the socket |
| `highland-daemon` | `DaemonError` | Process startup and runtime |
| `highland-cli` | `anyhow` | Command dispatch only |

The protocol crate carries more than one type because encoding, decoding, and
construction fail for genuinely different reasons, and a caller that only
encodes should not have to match decode failures.

A subsystem that produces domain values, such as `highland-vrrp`, MUST return a
typed error rather than a `Result` of `Option`, so that a caller can distinguish
"no address configured" from "the caller asked for the wrong family".

Context is preserved in the variant, not in a formatted string:

```rust
NetError::AddAddress {
    interface: String,
    address: IpCidr,
    source: io::Error,
}
```

- `R-23` Every error surfaced to a user MUST name the instance, the interface, the address
  or peer, and the underlying cause, in that order of specificity.
- `R-24` Errors MUST NOT be silently discarded. A swallowed error MUST be counted.

CLI output shape:

```text
failed to become MASTER for instance "api":
could not add VIP 192.0.2.10/24 to interface eth0: Operation not permitted

check:
  - process has CAP_NET_ADMIN
  - interface exists
  - address is not already assigned
```

---

## 19. Correctness Invariants

Each invariant is a named test requirement.

1. `I-35` Exactly one role is active per instance at any instant; a role change is atomic
   from the observer's point of view.
2. `I-14` A role other than `MASTER` never owns the VIP.
3. `I-36` `INIT`, `FAULT`, and `DISABLED` never transmit a normal advertisement.
4. `I-15` A node never becomes `MASTER` without a usable interface.
5. `I-04` A node never advertises `MASTER` without confirmed VIP ownership. There is no
   degraded-advertisement exception.
6. `I-37` Every ownership change emits an event.
7. `I-29` Every timer has a cancellation path.
8. `I-38` Health checks never block the state machine task.
9. `I-39` A malformed packet never terminates the daemon.
10. `I-09` A failed reload never alters the active configuration.
11. `I-31` Shutdown is idempotent.
12. `I-40` Repeated network errors produce bounded retries and bounded memory growth.
13. `I-26` A stale health result never overwrites a newer result.
14. `I-12` A stale generation result never modifies a newer instance.
15. `I-41` No task outlives its owning instance without explicit cancellation.
16. `I-42` An instance never removes another instance's VIP.
17. `I-43` A node never accepts an advertisement from a source that is not a configured peer.
18. `I-44` Repeated advertisements reset the master-down timer and nothing else.

---

## 20. Async Runtime and Concurrency

The runtime choice is a Milestone 0 decision recorded in `docs/adr/ADR-0002-async-runtime.md`
and is deliberately left open (Appendix B). Code samples in this document use Tokio for
readability; the following constraints hold regardless of the choice.

```text
supervisor
├── configuration task      # loads, validates, plans, applies generations
├── control API task        # authenticates, rate-limits, forwards
├── metrics task            # scrape endpoint
├── per-instance actor      # owns the state machine and all its timers
│   ├── protocol receive loop
│   ├── advertisement timer
│   ├── master-down timer
│   ├── health coordinator
│   └── network ownership executor
└── shutdown coordinator
```

- `R-25` Global mutable state and shared static mutability are forbidden. Each instance is
  an actor with an owned state machine, or an explicit event loop.
- `R-26` The state machine MUST remain synchronous and deterministic. Async code adapts
  external events into state-machine events and nothing else.
- `R-27` Timer duration functions MUST be pure so that fake-clock tests can assert exact
  fire times.

Conceptual loop:

```rust
loop {
    select! {
        Some(event) = protocol_rx.recv() => drive(event).await?,
        Some(event) = health_rx.recv()    => drive(event).await?,
        Some(event) = timer_rx.recv()     => drive(event).await?,
        Some(event) = control_rx.recv()   => drive(event).await?,
        Some(event) = netlink_rx.recv()   => drive(event).await?,
        _ = shutdown.cancelled()          => { drive(Event::ShutdownRequested).await?; break; }
    }
}

async fn drive(event: Event) -> Result<()> {
    let actions = machine.handle(event);
    for action in actions {
        match executor.apply(action).await {
            Ok(()) => {}
            Err(e) => { machine.handle(Event::ActionFailed { action: e.kind, error: e.into() }); }
        }
    }
    Ok(())
}
```

- `I-45` Executor failure MUST be fed back into the machine as an event, never logged and
  dropped.

---

## 21. Testing Strategy

Testing is a product feature. CI MUST run every tier below for the `0.x` line; the netns
and fuzz tiers are allowed to be nightly-only, but a failure on the default branch is a
release blocker.

### 21.1 Unit tests `[I]`

Packet encode/decode round trips, checksum, advertisement-interval arithmetic,
master-down arithmetic, priority and tie-break logic, preemption state, health aggregation,
state transitions, configuration validation (one test per `V-nn`), reload planning, and
event generation. All use a deterministic fake clock; any test using wall time or a real
random source is a defect.

### 21.2 Property tests `[I]`

Round-trip properties for arbitrary valid advertisements, rejection of arbitrary
truncated or mutated input, priority arithmetic properties, state-machine invariant
properties over random event sequences, and configuration normalization idempotence.

### 21.3 Fuzzing `[I]`

Targets: `fuzz_vrrp_ipv4_packet`, `fuzz_vrrp_ipv6_packet`, `fuzz_config_document`,
`fuzz_check_response`, `fuzz_netlink_message`, `fuzz_control_request`.

Each target MUST have bounded input, no network access, no filesystem access unless
required, a checked-in regression corpus for every fixed crash, and CI integration with
a smoke budget.

### 21.4 Network-namespace integration `[I]`

Build the two-node topology and assert, in order: node A becomes `MASTER`; node B remains
`BACKUP`; A fails; B becomes `MASTER`; the VIP moves; gratuitous ARP is observed; A
returns; preemption policy is honored; health failure demotes; packet loss does not cause
oscillation; concurrent startup resolves to exactly one `MASTER`; reload preserves
unaffected instances; timing budgets in §13.3 hold.

### 21.5 Chaos tests `[1]`

Dropped, delayed, duplicated, and reordered advertisements; link flaps; address-add and
address-remove failures; process pauses; clock jumps; slow checks; stale sockets; netlink
errors; simultaneous election. Each chaos scenario asserts bounded recovery time and
bounded event volume.

### 21.6 Compatibility tests `[1]`

Where practical, run Highland and Keepalived in isolated namespaces and verify both
directions of `MASTER`/`BACKUP`, IPv4 and IPv6 multicast, unicast mode, priority changes,
preemption, graceful shutdown, and advertisement-interval handling. The target is
documented as protocol compatibility only, never configuration compatibility.

---

## 22. Command-Line Interface `[I]`

### 22.1 Operations and commands

The control API is the single source of truth; the CLI maps onto it.

| Control operation | CLI | Destructive | Confirmation |
|---|---|---|---|
| `status` | `highland status [--json]` | no | no |
| `instances` | `highland instances [--json]` | no | no |
| `show <instance>` | `highland show <instance> [--json]` | no | no |
| `events` | `highland events [--follow] [--since] [--limit]` | no | no |
| `reload` | `highland reload [--yes]` | yes | unless `--yes` |
| `pause <instance>` | `highland pause <instance> [--yes]` | yes | unless `--yes` |
| `resume <instance>` | `highland resume <instance>` | no | no |
| `relinquish <instance>` | `highland relinquish <instance> [--yes]` | yes | unless `--yes` |
| `force-transition <instance>` | `highland force-transition <instance> --role <role> --enable` | yes | always, plus daemon flag `R-10` |
| — | `highland run --config <path>` | n/a | n/a |
| — | `highland check-config <path>` | no | n/a |
| — | `highland simulate --config <path>` | no | n/a |
| — | `highland import-keepalived --input --output` | no | n/a |
| — | `highland version` | no | n/a |

- `R-28` Destructive commands MUST show the target instance and its current role, MUST
  require confirmation unless `--yes` is given, and MUST produce an audit event naming the
  peer credential.
- `R-29` `force-transition` MUST be disabled unless the daemon is started with
  `--enable-force-transition`, and the CLI MUST refuse to issue it otherwise.

### 22.2 Transport rules `[I]`

A command that requires the daemon MUST fail with a clear message if the control socket is
unreachable; it MUST NOT fall back to reading the configuration file or talking to the
kernel.

---

## 23. systemd Integration `[I]`

```ini
[Unit]
Description=Highland VRRP failover daemon
Documentation=https://example.invalid/highland
After=network-online.target
Wants=network-online.target
Before=keepalived.service

[Service]
Type=notify
ExecStart=/usr/bin/highland run --config /etc/highland/config.toml
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=2s
AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
RuntimeDirectory=highland
StateDirectory=highland
ConfigurationDirectory=highland

[Install]
WantedBy=multi-user.target
```

- `R-30` `SIGHUP` MUST trigger the reload path of §10.5, with identical semantics to the
  control-socket `reload` operation.
- `R-31` The unit file is a starting point. `docs/user/operations.md` MUST explain capability
  and namespace requirements, and MUST state that `Before=keepalived.service` is only
  correct when co-running is intended.
- `R-32` The daemon MUST work with no systemd present, including under OpenRC or a bare
  container entrypoint.

---

## 24. Keepalived Compatibility `[1]`

Compatibility is incremental and additive.

### 24.1 Phase 1: protocol

Interoperate with standard VRRP peers, validate against packet captures, and support
standard IPv4 and IPv6 behavior.

### 24.2 Phase 2: configuration mapping

| Keepalived concept | Highland equivalent |
|---|---|
| `vrrp_instance` | `[[instance]]` |
| `interface` | `instance.interface` |
| `virtual_router_id` | `instance.vrid` |
| `priority` | `instance.priority` |
| `advert_int` | `instance.advertisement_interval` |
| `virtual_ipaddress` | `[[instance.vip]]` |
| `unicast_peer` | `instance.network.peers` |
| `nopreempt` | `instance.preempt = false` |
| `preempt_delay` | `instance.preempt_delay` |
| `track_script` | native checks, or an explicit command check |
| `notify_*` | event subscribers and the control API |

### 24.3 Phase 3: importer `[F]`

```text
highland import-keepalived \
  --input /etc/keepalived/keepalived.conf \
  --output /etc/highland/config.toml
```

The importer MUST warn about unsupported directives, MUST NOT silently drop
safety-relevant behavior, MUST emit a migration report, and MUST validate its own output
before writing it.

---

## 25. Documentation Requirements

| Document | Required content |
|---|---|
| `README.md` | What it is, scope tier, install, quick start, links |
| `docs/architecture.md` | Crate graph, state machine, tie-break rules, reason enum |
| `docs/user/index.md` | The entry point: what works today, and a routing table to everything else |
| `docs/user/getting-started.md` | Requirements, build, first configuration, two-node setup, running, reloading |
| `docs/user/installation.md` | Install paths, systemd, OpenRC, containers, permissions, uninstall |
| `docs/user/configuration.md` | Every key, every `V-nn`, a valid example per scope tier |
| `docs/user/health-checks.md` | Check types, weights, choosing a failure policy, avoiding flapping |
| `docs/user/cli.md` | Every subcommand, its flags, its defaults, and its current state |
| `docs/user/operations.md` | Network and firewall requirements, capabilities, split-brain behavior, capture diagnosis, VIP conflict diagnosis, every `reason` value, runbook |
| `docs/user/troubleshooting.md` | Symptom-first: from what is observed to its cause |
| `docs/user/upgrading.md` | Rolling upgrade, what a reload can and cannot change, rollback |
| `docs/user/threat-model.md` | `S-nn` coverage, privilege boundary, out-of-scope threats |
| `docs/user/compatibility.md` | Protocol compatibility claims and their limits, Keepalived mapping |
| `docs/testing.md` | How to run each test tier, netns prerequisites, fuzz workflow |
| `docs/adr/*` | Runtime choice, Netlink library choice, serialization format |
| `SECURITY.md` | Reporting path and supported versions |
| `CHANGELOG.md` | Keep a Changelog format, one section per release |

- `R-33` A change that adds a configuration key, a metric, a control operation, or a
  transition reason MUST update the corresponding document in the same commit.

---

## 26. Versioning and Stability

- Public crates use Semantic Versioning.
- Before 1.0: public APIs MAY change; configuration changes require a migration note in
  `CHANGELOG.md`; protocol behavior MUST remain standards-compliant; unstable features
  MUST be feature-gated and reported at startup.
- `R-34` A crate that another published crate depends on MUST NOT have a breaking change
  released without the dependents.

1.0 MUST NOT be declared until:

- IPv4 and IPv6 VRRPv3 are interoperable with at least one existing implementation
- Multicast and unicast modes work
- VIP ownership is reliable in the netns suite and in a long-running deployment
- The netns integration suite is stable across at least 30 consecutive CI runs
- Fuzzing is established with a checked-in corpus
- Reload is transactional and proven safe
- Upgrade and rollback procedures are documented
- Failure and split-brain behavior is documented and well understood
- At least one extended real-world deployment has run

---

## 27. Milestones

Each milestone lists exit criteria that MUST be objectively checkable.

### Milestone 0 — Repository and architecture

Workspace layout, CI, `cargo fmt` and `cargo clippy` enforcement, documentation skeleton,
error conventions, test conventions, feature flags, MSRV policy, ADR scaffolding.

Exit (`M-01`): `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
warnings`, `cargo test --workspace`, `cargo deny check`, and `cargo audit` all pass on the
default branch.

### Milestone 1 — Pure state machine

Roles, events, timers, priority logic, tie-break, preemption, health aggregation, fake
clock, property tests. No Linux networking.

Exit (`M-02`): every `I-nn` in §11 and §19 is covered by at least one test, and
`highland-core` compiles and tests with no runtime dependency. Three of those
invariants are only partly owned by the state machine, and the milestone records
which part is tested here and which part is tested later:

| Invariant | Owned by | Tested in |
|---|---|---|
| `I-19` confirmation means read-back | the executor | Milestone 1 tests the state machine's share: `ActionSucceeded` is the only path to ownership. Milestone 3 tests the read-back itself |
| `I-09` a rejected reload changes nothing | the reload planner | Milestone 1 tests the generation guard (`I-12`); Milestone 4 tests the whole-reload behaviour |
| `I-39` a malformed packet never terminates the daemon | the decoder | Milestone 2, with the fuzz targets |

A coverage audit is a review step, not a claim: `crates/highland-core/tests/`
names each invariant it enforces in a test name, so an invariant without a test
is visible in a diff.

### Milestone 2 — VRRP packet implementation

Encoding, decoding, checksum, validation, IPv4 and IPv6 tests, fuzz targets, capture
fixtures.

Exit (`M-03`): fuzz targets run in CI for a defined smoke budget with no crash; captures
in `tests/packet-captures/` decode successfully.

### Milestone 3 — Single-instance Linux daemon `[I]`

One interface, one VRID, IPv4, unicast, VIP add and remove, basic transitions, structured
logs.

Exit (`M-04`): a single-instance daemon fails over a VIP against a scripted second node in
a namespace; all `V-nn` for the `[I]` key set are enforced.

### Milestone 4 — Two-node integration `[I]`

Namespace harness, master-failure tests, VIP movement, gratuitous ARP, configuration
validation, graceful shutdown, control socket, metrics, reload.

Exit (`M-05`): the §21.4 scenario passes deterministically in CI, reload is transactional,
and timing budgets in §13.3 are asserted. This milestone is the **0.1.0 initial public
release**.

### Milestone 5 — IPv6 and multicast maturity `[1]`

IPv6 VIPs and advertisements, unsolicited Neighbor Advertisements, multicast mode for both
families, multiple and mixed-family peers, link and route diagnostics.

Exit (`M-06`): IPv4 and IPv6 multicast and unicast suites pass, and the IPv6 half of `G-01`
is met.

### Milestone 6 — Native health checks `[1]`

TCP, HTTP, HTTPS, Unix socket, interface checks, thresholds, weighted priority, explainable
demotion.

Exit (`M-07`): every check type has unit, netns, and failure-mode tests; `I-23` through
`I-26` hold.

### Milestone 7 — Operations interface `[1]`

Control socket, status API, metrics, reload, pause and resume, relinquish, event stream.

Exit (`M-08`): `R-21` through `R-29` hold; metric label cardinality is bounded by test.

### Milestone 8 — Compatibility and hardening `[1]`

Keepalived interoperability, chaos tests, security review, resource limits, capability
documentation, packaging.

Exit (`M-09`): every `L-nn` is asserted by a test; `docs/user/threat-model.md` covers every
`S-nn`; compatibility tests pass in both directions.

### Milestone 9 — 1.0 candidate `[1]`

Stable configuration subset, stable crate APIs, upgrade documentation, release artifacts,
long-running deployments, incident playbooks.

Exit (`M-10`): every §26 condition is satisfied and recorded in `CHANGELOG.md`.

---

## 28. Example Library API

These examples show the intended shape and are the API the crates are built
towards. They are illustrative rather than frozen: the conceptual separation is
normative, the exact signatures are not. Where the code already implements a
signature, this section matches it; where a milestone is still to come, the
example states so.

```rust
use highland_core::clock::ManualClock;
use highland_core::state::{Action, Event, InstanceConfig, InstanceStateMachine, Role, TimerId};
use std::time::Duration;

let clock = ManualClock::new();
let config = InstanceConfig {
    name: "api".to_owned(),
    vrid: 42,
    priority: 150,
    ..InstanceConfig::default()
};

let mut machine = InstanceStateMachine::new(config, clock.clone());
let actions = machine.handle(Event::Startup);

assert_eq!(machine.role(), Role::Backup);
assert!(actions.contains(&Action::ArmTimer {
    timer: TimerId::MasterDown,
    deadline: Duration::from_millis(3010),
}));

// Timers are absolute deadlines, so a test asserts the deadline rather than
// sleeping.
clock.advance(Duration::from_millis(3010));
assert!(machine.is_due(TimerId::MasterDown));
```

Protocol usage, once Milestone 2 lands the codec. The field values are validated
on construction, so an `Advertisement` that exists can be encoded:

```rust
use highland_vrrp::{Advertisement, IpFamily, Priority, Vrid};
use std::time::Duration;

let advertisement = Advertisement::new(
    Vrid::new(42)?,
    Priority::new(150)?,
    Duration::from_secs(1),
    vec!["192.0.2.10".parse()?],
)?;
assert_eq!(advertisement.family(), IpFamily::V4);

// Arrives with Milestone 2:
// let bytes = encode_advertisement(&advertisement, IpFamily::V4)?;
// let decoded = decode_advertisement(&bytes, IpFamily::V4)?;
// assert_eq!(decoded.vrid().get(), 42);
```

Health-check usage. The debouncer is the part with interesting semantics: a
single failed probe never changes the verdict unless the failure threshold is
one.

```rust
use highland_checks::{CheckKind, CheckResult, CheckSpec, CheckStatus, Debouncer, Stability};
use highland_core::state::Generation;
use std::time::{Duration, Instant};

let spec = CheckSpec::new(
    "database",
    CheckKind::Tcp,
    Duration::from_secs(2),
    Duration::from_millis(500),
    3,
    2,
    100,
)?;

let mut debouncer = Debouncer::new(&spec);
assert_eq!(debouncer.observe(CheckStatus::Passing), None);
assert_eq!(debouncer.observe(CheckStatus::Passing), Some(Stability::Passing));

// A result carries the sequence number that makes out-of-order delivery
// harmless: a stale result can never overwrite a newer verdict.
let older = CheckResult::passing(
    "database",
    Duration::from_millis(3),
    Instant::now(),
    Generation::initial(),
    1,
    "connection accepted",
)?;
let newer = CheckResult::passing(
    "database",
    Duration::from_millis(2),
    Instant::now(),
    Generation::initial(),
    2,
    "connection accepted",
)?;
assert!(newer.supersedes(&older));
```

## 29. Failure Scenarios the Design Must Handle

### 29.1 MASTER loses its application

HTTP check fails; `failure_threshold` is reached; under `fail_closed` the node becomes
ineligible and emits `health_ineligible`; it sends a zero-priority advertisement, removes
its VIPs, and enters `BACKUP`; the `BACKUP` takes over within one master-down interval;
clients receive gratuitous ARP. Under `weighted`, the node instead advertises a reduced
priority and steps down only when a higher-priority peer advertises (`R-16`).

### 29.2 MASTER loses the interface

Link event received; advertisement task stops; VIPs are removed if the kernel permits; the
`BACKUP` detects advertisement timeout and becomes `MASTER`; the failed node enters `FAULT`
and will not claim ownership until hold-down expires and re-verification succeeds.

### 29.3 A higher-priority node returns

With `preempt = true`: startup delay completes, one health evaluation window passes,
preemption delay starts, the returning node advertises, the incumbent steps down per
`R-16`, the VIP moves, and gratuitous ARP is sent. With `preempt = false`: the returning
node stays `BACKUP` and the incumbent keeps the VIPs.

### 29.4 Both nodes start simultaneously

Election converges to exactly one `MASTER` via §12.3; the loser logs the winner's address
and the rule that decided it; neither node claims ownership without advertising.

### 29.5 Network partition

Both nodes may become `MASTER`. Highland reports peer loss on both sides, makes no claim of
consensus, exposes role history and peer state for inspection, and documents the exposure.
Fencing is `[F]`.

### 29.6 VIP addition fails

The node does not report `MASTER`, enters `FAULT`, retries with bounded backoff, reports
interface, address, and kernel error, attempts cleanup of partially added addresses, and
leaves other instances untouched.

### 29.7 Clock jumps

Timers are computed from a monotonic clock abstraction. A wall-clock jump MUST NOT change
timer behavior, and health check scheduling MUST remain interval-based rather than
absolute-time-based.

---

## 30. Performance Requirements

Highland is infrastructure software, not a packet-processing engine. Correctness and
predictable behavior come first.

- Advertisement handling MUST NOT dedicate a thread or task per packet.
- Health checks MUST have an explicit concurrency limit (`L-07`).
- A node MUST support at least 64 instances without pathological behavior.
- Invalid packet floods MUST NOT cause unbounded CPU or memory use (`L-11`).
- Metrics MUST NOT create unbounded label cardinality (`L-10`).
- Reload MUST NOT interrupt unaffected instances (`I-10`).

Benchmark, in CI as trends rather than gates: packet encode and decode throughput,
state-machine event throughput, configuration parse time, health scheduler overhead,
netlink latency, metrics overhead, and behavior at the `L-04` instance limit.

---

## 31. Design Principles

- `D-01` The protocol implementation stays independent from Linux.
- `D-02` The state machine stays independent from the async runtime.
- `D-03` Network side effects live behind traits and explicit executors.
- `D-04` Invalid states are difficult to represent.
- `D-05` Typed configuration over stringly typed behavior.
- `D-06` Ownership changes are never hidden.
- `D-07` Shell execution is never the default.
- `D-08` Failure explanations are first-class.
- `D-09` Tests and packet captures are part of the product.
- `D-10` A smaller correct feature beats a broader unreliable one.
- `D-11` Never promise fencing that VRRP cannot provide.
- `D-12` Make behavior observable before adding capability.
- `D-13` Prefer removing an ambiguous knob over documenting its ambiguity.

---

## 32. Definition of Done for the First Public Release

`0.1.0` is ready when:

- A two-node Linux deployment fails over an IPv4 VIP reliably
- Unicast peer mode works with multiple peers
- Health checks trigger controlled demotion, explainably
- State transitions are visible in logs, metrics, and the status API
- Configuration errors are clear, located, and actionable
- Netns integration tests pass deterministically
- Fuzz targets exist for every untrusted parser
- The daemon survives malformed packets and repeated network errors
- Graceful shutdown removes ownership correctly
- Split-brain limitations are documented
- A failed reload leaves the previous configuration active
- A user can install and operate it without reading the source
- No core feature requires executing a shell command

## 33. Project Definition

Highland is:

> A Rust-native Linux high-availability networking library and daemon that implements
> VRRP-based virtual IP failover with typed configuration, native health checks,
> deterministic state transitions, and first-class observability.

It is not marketed as a complete Keepalived clone. Its identity is:

> **A safe, explainable, testable failover engine for Linux.**

---

## Appendix A — Resolved Ambiguities

Decisions made while consolidating the draft. Each entry names the contradiction and the
resolution, so a later change can be traced.

| # | Contradiction | Resolution |
|---|---|---|
| A-01 | Goals and platform sections promised IPv4 and IPv6 and multicast; the recommended initial scope promised IPv4 and unicast only | Explicit three-tier scope model (§3); every requirement is tagged |
| A-02 | `failure_policy = "degrade"` and `"weighted"` described the same arithmetic | `degrade` removed and merged into `weighted`; enum is `fail_closed`, `weighted`, `manual` |
| A-03 | `CheckCriticality` (advisory/weighted/critical) and per-check `weight` both expressed check importance | `CheckCriticality` removed; `weight = 0` means observational, and `all_checks_required` extends the `fail_closed` trigger |
| A-04 | `all_checks_required` had no defined meaning | Defined as "under `fail_closed`, any failing check blocks ownership" (§12.2) |
| A-05 | An invariant required `MASTER` to own all VIPs, another allowed "degraded advertisement" if configured | Degraded advertisement removed; ownership-before-advertise is a hard invariant (`I-04`, §5) |
| A-06 | Example config combined `preempt = false` with `preempt_delay`, which the same document's rules forbid | Example corrected; `V-10` enforces it |
| A-07 | Example config gave one instance both IPv4 and IPv6 VIPs and one flat unicast peer list, while validation rejected mixed families | Per-family peer inference, and `V-03` forbids mixed families in `0.x` |
| A-08 | `V-07` read "a unicast peer is unspecified" | Rewritten as "a VIP family has no configured peer of that family" |
| A-09 | Tie-breaking was required to "be documented" but no rule was given | Deterministic four-step rule (§12.3) |
| A-10 | Timer calculations were tested but never specified | Timer table and timing budgets specified (§13, §13.3) |
| A-11 | Health-driven relinquish was "according to policy" | Explicit per-policy relinquish table (§12.4) |
| A-12 | The decoder was said to reject "packets violating configured peer rules" | Peer filtering moved to the caller; `highland-vrrp` has no peer concept (§9.2) |
| A-13 | The control API had `health` and `force-transition`; the CLI had `simulate` and `version` but no `health` | One operation table; `health` folded into `status`/`show`; CLI adds `run`, `check-config`, `simulate`, `version`, `import-keepalived` (§22.1) |
| A-14 | The event enum had no operator events, though the control API supports pause, resume, and relinquish | `Operator*Requested` events added, with audit requirements (`R-09`, `R-10`) |
| A-15 | `Role` had `Fault` and `Disabled` but the terminology table did not | Added to §6 with per-role ownership and timer rules (§11.1) |
| A-16 | Executor failure was required to be reported to the machine but no mechanism existed | `Event::ActionFailed` added, with an executor contract table (§11.4, `I-45`) |
| A-17 | The event loop used `tokio::select!` while the runtime was undecided | Runtime left open in Appendix B; `select!` shown generically, with `highland-core` forbidden from depending on any runtime |
| A-18 | Retry behavior was "bounded" without a bound | Backoff base 1s, cap 30s, 10 attempts (§11.5) |
| A-19 | Metrics had no types and no role encoding | Types, labels, and the role encoding are specified (§16.2) |
| A-20 | `[node].instance_id` appeared in the example but was never defined or validated | Removed |
| A-21 | Duplicate-VRPID detection was described vaguely as "conflict on the same interface and network context" | Strict uniqueness of `(interface, vrid)` (`V-06`) |
| A-22 | Several limits existed only as prose | Consolidated into `L-01` through `L-15` with values (§17.4) |
| A-23 | Requirements were not traceable to tests | `I-nn`, `V-nn`, `S-nn`, `L-nn`, `M-nn` identifiers with a stated test obligation (§0.2) |
| A-24 | Milestones had no exit criteria | Each milestone has numbered, checkable exit criteria (§27) |
| A-25 | The systemd unit had no reload path | `ExecReload` with `SIGHUP` mapped onto the reload path (`R-30`) |
| A-26 | `docs/SPEC.md` and ADRs were absent from the workspace listing | Added (§8) |
| A-27 | No rule required an instance to exist or a node name to be set | Added `V-31` and `V-32` during Milestone 0 |
| A-28 | `V-20` rejected a total check weight of 0, which would forbid an instance from having only observational checks | `V-20` now rejects only a total above 255 |
| A-29 | §18 sketches one `HighlandError` aggregating every subsystem, but the dependency graph gives the CLI no access to the network or check errors | Each crate owns one error type; see `docs/architecture.md` |
| A-30 | §8 places tests at the workspace root, where cargo cannot compile them | Integration tests live in `crates/<crate>/tests/`; `tests/` holds shared fixtures |
| A-31 | §16.1 and §16.3 showed a free-form JSON shape with top-level fields, which no typed model can produce | The event and status shapes are defined as typed messages with a fixed field set and a scalar value union; the examples match them |
| A-32 | §9 listed `highland-net` as depending on the core but not on the protocol crate, although address ownership and advertisement sending need the codec | `highland-net` may depend on `highland-vrrp`; the edge appears when Milestone 3 lands |
| A-33 | §11.2 had no way for the machine to learn that an action had **succeeded**, so a takeover had to be optimistic and `I-04` was a matter of action ordering | Added `Event::ActionSucceeded`. Ownership is now confirmed before `MASTER` is entered, which makes the invariant structural |
| A-34 | §11.4 sent a failed `RemoveVirtualAddresses` to `FAULT`, which contradicts `I-14`: the addresses are still present, so a non-master role would own them | A failed removal keeps the role at `MASTER` with advertising stopped, and retries with backoff; the role changes once removal is confirmed |
| A-35 | Preemption and administrative interface management had no transition reason and no event | Added `preemption_delay_elapsed` to §16.1.1 and `Event::InterfaceBroughtUp`, so an interface may be brought down again only once the instance is not `MASTER` |
| A-36 | `M-02` demanded that every `I-nn` in §11 and §19 be tested, which is not possible for invariants a later milestone owns | The exit criterion now names the three partly-owned invariants and the milestone that finishes each |
| A-37 | §8 and §25 listed the operator documents at the top of `docs/`, which made them sit beside developer documents and spread one topic across several files | End-user documentation is consolidated under `docs/user/`, with an index as its entry point. The specification points there |

## Appendix B — Open Questions

These are deliberately undecided. Each is an `R-nn`-level blocker for the milestone named
and MUST be resolved in an ADR before the dependent work starts.

| # | Question | Needed by | Default if unresolved |
|---|---|---|---|
| B-01 | Which async runtime? | Milestone 0 | None. Blocks `R-25` implementation |
| B-02 | Which Netlink library, or direct `rtnetlink` use? | Milestone 3 | None. Blocks `R-04` |
| B-03 | Multicast implementation: one socket per family per instance, or per interface with demultiplexing? | Milestone 5 | Per-family sockets, demultiplexed by VRID |
| B-04 | Does `RUST_LOG`-style filtering apply to metrics and events, or only logs? | Milestone 7 | Logs only |
| B-05 | Serialization library for the control API and event stream? | Milestone 7 | Serde JSON |
| B-06 | Whether `simulate` ships in `1.0` as a subcommand or stays a test binary | Milestone 7 | Test binary only |
| B-07 | Whether hold-down and retry parameters become configurable or stay fixed | Milestone 4 | Fixed, as specified in §11.5 |
