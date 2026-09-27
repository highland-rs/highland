# Architecture

This document describes how the crates fit together, why the dependency edges
run the way they do, and which specification requirement each design choice
serves. The normative behavior lives in [`SPEC.md`](SPEC.md); this document
explains the shape of the implementation.

## Design rules

1. The protocol implementation knows nothing about Linux.
2. The state machine knows nothing about the async runtime.
3. Network side effects live behind traits and explicit executors.
4. Invalid states are hard to represent.
5. Nothing is hidden: ownership changes and failures are always explained.

These are `D-01` through `D-13` in `SPEC.md` §31.

## Crate graph

```text
highland-cli        highland-daemon
     |                    |
     |                    +---- highland-observe
     |                    +---- highland-control
     |                    +---- highland-config
     +---- highland-control        |
     +---- highland-config         +---- highland-net
                                   +---- highland-checks
                                   +---- highland-core
                                   +---- highland-vrrp
```

Rules:

- A crate may depend only on crates listed below it in `SPEC.md` §9. Cycles are
  forbidden.
- `highland-core` and `highland-vrrp` depend on nothing. That is what makes the
  state machine and the codec testable without Linux, without a network, and
  without a runtime.
- Configuration does not depend on checks. Checks are built from configuration,
  not the other way round, which keeps the configuration layer free to be reused
  by importers and by the control API.

### The dependency graph as built in Milestone 0

Crates declare only the dependencies they actually use, which is a subset of the
target graph in `SPEC.md` §9:

| Crate | Declares |
|---|---|
| `highland-core` | `thiserror` |
| `highland-vrrp` | `thiserror` |
| `highland-net` | `thiserror`, `highland-vrrp`, and on Linux `rtnetlink`, `socket2`, `tokio` |
| `highland-checks` | `highland-core`, `thiserror` |
| `highland-config` | `serde`, `thiserror`, `toml` |
| `highland-observe` | `serde`, `tracing`, `thiserror` |
| `highland-control` | `serde`, `serde_json`, `thiserror` |
| `highland-daemon` | `anyhow`, `thiserror`, `tokio`, `tracing`, `tracing-subscriber`, `highland-core`, `highland-config`, `highland-checks`, `highland-control`, `highland-net`, `highland-observe`, `highland-vrrp` |
| `highland-cli` | `anyhow`, `clap`, `serde_json`, `tokio`, `highland-config`, `highland-control` |

`highland-net` is the only crate with a platform-gated dependency. `rtnetlink`
cannot compile off Linux at all, so it is declared under
`[target.'cfg(target_os = "linux")'.dependencies]` and every other platform gets
`UnsupportedBackend`, whose operations return `NetError::Unsupported`. The
executor and its tests therefore run on a contributor's Mac while the kernel
talk stays on Linux. The reasoning is in
[`docs/adr/ADR-0003-netlink-library.md`](adr/ADR-0003-netlink-library.md).

Edges that the target graph does not have yet, and when they appear:

- `highland-checks` → `highland-config`, when the check builder reads typed
  configuration (Milestone 6).
- `highland-observe` → `highland-core`, when metrics and events carry
  `Generation` and `Role` directly (Milestone 7).
- `highland-net` → `highland-vrrp`, when the socket layer encodes and decodes
  advertisements (Milestone 3).

## The state machine

`highland-core` owns role. The state machine is synchronous, deterministic, and
performs no I/O: it consumes `Event` values and returns `Action` values.

```text
external event ──▶ InstanceStateMachine::handle ──▶ Vec<Action> ──▶ executor
                          │                            │
                          └── role, reasons             └── outcome
                                                    ┌───────┴───────┐
                                     ActionSucceeded│               │ActionFailed
                                                    ▼               ▼
                                          enters MASTER          FAULT or retry
```

Both outcomes are reported, not just failures. A machine told only about
failures would have to enter `MASTER` optimistically, which is exactly the shape
that lets a node advertise an address it does not hold. Reporting success is what
makes `I-04` structural: the only route from `BACKUP` to an advertisement runs
through `Event::ActionSucceeded { kind: AddAddresses }`.

`Action` and `ActionKind` are separate types because the kind is the coarse
classification used when an outcome comes back. Adding an action must not require
a new outcome path.

### Where the modules sit

| Module | Owns |
|---|---|
| `state` | The vocabulary: `Role`, `Event`, `Action`, `TimerId`, `Generation`, `TransitionReason`, `InstanceConfig` |
| `machine` | The transitions, and the pending-ownership handshake |
| `timer` | `TimerSet` and `RetryPolicy` |
| `health` | The health and effective-priority arithmetic |
| `election` | The four-step tie-break |
| `clock` | `Clock`, `ManualClock`, `Rng` |

Separating the vocabulary from the logic is deliberate: a reader can learn what
the machine can say without first reading what it does.

### The protocol layer

`highland-vrrp` follows RFC 5798, and the RFC is the authority: where the code
and the specification disagree, the code is a bug. Implementing against the
specification text rather than from memory turned up four errors in the
specification itself, recorded in `SPEC.md` Appendix A:

- the advertisement interval is a **12-bit** centisecond field, not an 8-bit one,
  so the configured range is 10ms to 40.95s rather than 10ms to 2.55s;
- `Skew_Time` is `((256 - priority) * Master_Adver_Interval) / 256`, not a
  constant allowance, so the takeover delay is between three and four intervals;
- a backup must **discard** a lower-priority advertisement rather than reset its
  timer, which is what makes preemption converge;
- there is one message format for both address families, with a 4-bit reserved
  field sharing an octet with the interval.

Decoding is two-phase. `Peek::read` parses the eight fixed octets without
trusting the count, so a receiver can check the TTL, the source address, and the
peer list first; the addresses are read only after the checksum verifies. A
decoder that sized an allocation from an unauthenticated count field would be a
denial-of-service vector, and the fuzz targets assert that it never does.

The checksum scope is an explicit parameter. RFC 5798 §5.2.8 requires an RFC 2460
pseudo-header without distinguishing the families, and interoperating
implementations compute the plain message checksum for IPv4. Rather than pick one
silently, `ChecksumScope` makes the choice visible, and its IPv6 default is
`Undecidable`: producing a checksum without the addresses would produce one that
fails only in the field, so it refuses. Milestone 8 settles the IPv4 case against
a real implementation.

### Timers

Deadlines are absolute, not relative. A relative delay would make the machine's
output depend on when the executor applied the previous action, which would
break determinism (`R-27`). A test therefore asserts a deadline:

```rust
clock.advance(Duration::from_millis(3410));
assert!(machine.is_due(TimerId::MasterDown));
```

Firing is an explicit event, not a side effect of time. A delivered timer event
for a timer that is not armed is ignored, which is what stops a stale delivery
from reviving an instance that has stopped participating (`I-30`).

Time enters only through `Clock`. `ManualClock` exists so a test can assert an
exact deadline:

```rust
let clock = ManualClock::new();
let mut machine = InstanceStateMachine::new(config, clock.clone());
machine.handle(Event::Startup);
assert_eq!(machine.deadline_of(TimerId::MasterDown), Some(Duration::from_millis(3410)));
```

`Master_Down_Interval` is `3 * adver_int + 10ms`, from RFC 5798. The state
machine uses a saturating form because the interval has already been bounded by
`V-04`; the fallible `master_down_interval` is the public API for callers that
have not validated their input.

## Errors

`SPEC.md` §18 sketches a single `HighlandError` aggregating every subsystem.
That is not what the code does, and the reason is the dependency graph: the CLI
depends on `highland-config` and `highland-control` only, so an aggregate that
mentions `NetworkError` and `CheckError` would force the CLI to depend on
`highland-net` and `highland-checks` for no benefit.

Each crate therefore owns one error type:

| Crate | Error |
|---|---|
| `highland-core` | `CoreError` |
| `highland-vrrp` | `ProtocolError`, `EncodeError`, `DecodeError`, `AdvertisementError` |
| `highland-net` | `NetError` |
| `highland-checks` | `CheckError` |
| `highland-config` | `ConfigError` |
| `highland-control` | `ControlError` |
| `highland-daemon` | `DaemonError` |

The invariants that matter are preserved: libraries use `thiserror`, variants
carry structured fields, and every message a user sees names the instance, the
interface, the address, and the cause (`R-23`).

## Configuration

Loading is three steps so a failure is attributable:

1. read the bytes, enforcing the size limit and the permission check (`L-06`,
   `V-26`),
2. parse into the typed model, rejecting unknown keys,
3. validate semantically, collecting every `V-nn` violation.

Every semantic rule lives in `highland-config/src/validation.rs` and has a named
test in `crates/highland-config/tests/validation.rs`. A rule without a test is a
defect; `every_implemented_rule_has_a_test` checks the two lists against each
other.

Durations are written with an explicit unit. `1000` is rejected rather than
assumed to mean a thousand seconds.

Unknown configuration keys are an error, not a warning. A typo that silently does
nothing is exactly the failure mode typed configuration exists to prevent.

## Observability

Events are values. `EventName` is a closed enum because dashboards and alerting
depend on the spelling (`R-17`). The event ring is bounded at 4096 entries and
counts what it drops, so a flood costs memory nothing and hides nothing
(`L-08`).

Redaction happens at the observability boundary, not at each call site, so a
careless call site cannot leak a secret into a log (`S-01`).

## Concurrency

The async runtime is not chosen yet (`B-01`). The code so far is runtime-agnostic
except for the two binaries and `highland-cli`'s socket client, which use Tokio.
`highland-core` and `highland-vrrp` do not depend on any runtime, and that is
asserted by the dependency table above rather than by a comment.

Per-instance actors, the executor contract, and the event loop are specified in
`SPEC.md` §20 and §21 and arrive with Milestone 1 and 3.

## Feature flags

| Feature | Default | Meaning |
|---|---|---|
| `command-checks` (in `highland-checks`, mirrored in `highland-daemon`) | off | Allows `type = "command"` checks. Requires an explicit allow-list in configuration as well (`R-06`, `V-21`) |
| `netns-tests` (planned) | off | Enables tests that create network namespaces and need root (`R-01`) |

A `[F]` feature MUST be behind an explicit default-off flag, and the daemon MUST
run correctly with every `[F]` feature disabled (`SPEC.md` §3.4).

## The executor and the actor

`highland-core` decides; `highland-daemon` acts. The seam is the [`Executor`],
which takes one `Action` at a time and answers with the event to feed back.

The loop that applies actions is a **worklist**, not a single pass. That is not a
detail: confirming the addresses is what makes the machine enter `MASTER` and ask
to advertise, so the actions produced by the *outcome* of an action have to be
applied too. A single pass leaves a master that owns its address and never says
so. `InstanceActor::handle` drains the worklist, bounded at sixteen rounds,
because a machine that never settles would otherwise spin.

`InstanceActor` owns one machine and one executor, and `run_instance` owns the
`select!` over protocol events, the earliest timer deadline, and shutdown. There
is no shared mutable state between instances, which is the property `SPEC.md` §20
exists to protect.

The state machine's actions that touch the kernel or the wire are confirmed;
timers, roles, events, and the effective priority are the machine's own
bookkeeping and answer with `None`. Everything else that could fail answers with
`ActionFailed`, so the machine never assumes an outcome.

## What Milestone 3 does and does not do

Delivered and tested: the Netlink backend with read-back confirmation, the VRRP
transport's validation and peer filtering, the executor, the actor, the run loop,
and a full failover exercised end to end against a scripted kernel with no
privileges.

Not delivered: the raw socket. Putting an advertisement on the wire needs a
`SOCK_RAW` socket for IP protocol 112, and reading a peer's TTL back off an
incoming datagram needs ancillary data. Both are Linux-specific and neither can be
developed or verified on the machine this milestone was built on, so the daemon
refuses to start (`TRANSPORT_AVAILABLE` is `false`) rather than running a process
that claims to be a VRRP router while sending nothing. The socket and the
namespace harness land together in Milestone 4.

## What Milestone 1 does not do

- Nothing applies the machine's actions. `highland-net` has the trait, not an
  implementation, so no address is added and no advertisement is sent.
- The control socket has a message model, not a listener.
- Reload planning is not implemented; the state machine only enforces the
  generation guard (`I-12`), which is the part that protects an instance from a
  stale configuration.

## What Milestone 0 deliberately does not do

- No VRRP packets are encoded or decoded.
- No sockets are opened, no addresses are added, no netlink is spoken.
- The control socket has a message model but no listener.
- The daemon starts, loads its configuration, waits for a signal, and stops.
