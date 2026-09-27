# Changelog

All notable changes to Highland are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `docs/SPEC.md`: the normative specification, with scope tiers
  (`[I]` initial release, `[1]` version 1.0, `[F]` post-1.0), RFC 2119 language,
  and stable requirement identifiers (`G`, `R`, `I`, `V`, `L`, `S`, `M`, `D`).
- A nine-crate workspace matching the dependency rules in `SPEC.md` §9.
- `highland-core`: `Role`, `Event`, `Action`, `TimerId`, `Generation`,
  `TransitionReason`, `InstanceStateMachine`, and the `Clock` and `Rng`
  abstractions, with a deterministic `ManualClock`.
- `highland-vrrp`: `Version`, `Vrid`, `Priority`, `IpFamily`, and `Advertisement`,
  validated on construction so an invalid wire value cannot be represented.
- `highland-net`: the `NetworkBackend` trait, typed interface and CIDR types, and
  `NetError` with structured context.
- `highland-checks`: `CheckSpec`, `CheckResult` with sequence numbers, the
  `Stability` debouncer, and the `Check` trait. The `command-checks` feature
  exists and is off by default.
- `highland-config`: the typed configuration model, a strict TOML parser with
  explicit-unit durations, and the `V-01` through `V-32` validation rules, each
  with its own test.
- `highland-observe`: the event model, the redaction layer, and a bounded event
  ring.
- `highland-control`: the control request and response messages, the error
  taxonomy, and a token-bucket rate limiter.
- `highland-daemon` and `highland-cli` binaries, with a working
  `check-config` command and a `run` command that delegates to the daemon binary.
- Continuous integration covering formatting, lints, tests, dependency policy,
  advisories, and the MSRV.
- Project documentation skeleton: architecture, configuration, operations,
  threat model, compatibility, testing, and three ADRs.

### Changed

- `highland-core` is now the real state machine rather than a skeleton. It adds
  `timer` (a `TimerSet` and a `RetryPolicy`), `health` (the policy arithmetic of
  `SPEC.md` §12.2), `election` (the four-step tie-break of §12.3), and `machine`
  (the full `InstanceStateMachine`).
- Ownership is now a two-phase handshake: the machine requests addresses and
  enters `MASTER` only on `Event::ActionSucceeded`. There is no path from
  `BACKUP` to an advertisement without confirmed ownership, which makes `I-04`
  structural rather than a matter of action ordering.
- A failed address removal keeps the instance `MASTER` with advertising stopped,
  because the addresses are still present. The role changes once removal is
  confirmed.
- A shutdown or a pause cancels an in-flight ownership request and owes a
  best-effort removal, so a late confirmation cannot revive an instance.

### Added in this milestone

- `highland-net` has a real Linux backend: interface lookup with addresses, link
  state, address add and remove **confirmed by read-back** rather than by
  acknowledgment, and a link/address subscription (`I-19`).
- `highland-net::vrrp`: the receiver-side rules, in the order a receiver should
  apply them. The TTL must be 255, the source must be a configured peer, the VRID
  must match, and the length must agree with the count before anything is
  allocated from it.
- `highland-net::ScriptedBackend`: a scriptable kernel, which is what `R-03` asks
  for and what makes the failure paths testable without privileges.
- `highland-daemon::Executor`, which applies one action and answers with the
  outcome, and `Transport`, so the wire can be swapped without touching the
  failover logic.
- `highland-daemon::InstanceActor` and `run_instance`: one task per instance, no
  shared state, one timer for the earliest deadline rather than six tasks.
- 15 daemon tests, including the whole failover driven end to end: startup,
  takeover, ownership confirmed before advertising, a failed add faulting instead
  of claiming the address, relinquishment, shutdown, and a paused instance
  staying out.

### Changed

- `NetworkBackend` is now `async`. Netlink is an asynchronous socket, and a
  synchronous trait would have forced a blocking wrapper on the runtime thread,
  which is exactly what `I-38` forbids. A scripted backend is unaffected: its
  futures are already complete.
- The action loop is a **worklist**, not a single pass. Confirming the addresses
  is what makes the machine enter `MASTER` and ask to advertise, so the actions
  produced by an outcome must be applied too. A single pass left a master that
  owned its address and never said so; the failover test caught it.

### Not delivered, and why

- **The raw VRRP socket.** It needs `SOCK_RAW` for IP protocol 112, and reading a
  peer's TTL back needs ancillary data. Both are Linux-specific, and this
  milestone was built on macOS, where neither can be compiled or verified. The
  daemon therefore refuses to start rather than run a process that claims to be a
  VRRP router while sending nothing (`TRANSPORT_AVAILABLE` is `false`).
- **Gratuitous ARP**, which needs an `AF_PACKET` socket whose `sockaddr_ll` has no
  safe representation. It returns `NetError::Unsupported`, and the state machine
  already treats that failure as non-fatal.
- **The namespace harness**, which is Milestone 4's exit criterion and needs root.
  What the scripted-kernel tests establish is that the logic and the sequencing
  are right, so that the namespace run has one variable rather than two.

### Earlier in this milestone

- **The advertisement interval is a 12-bit centisecond field** (RFC 5798
  §5.2.7), not the 8-bit field the specification assumed, so the accepted range
  is 10ms to 40.95s. `V-04` and the configuration bound were both wrong.
- **`Skew_Time` is not a constant.** RFC 5798 §6.1 defines it as
  `((256 - priority) * Master_Adver_Interval) / 256`, and
  `Master_Down_Interval` as `3 * Master_Adver_Interval + Skew_Time`. The takeover
  delay is therefore between three and four intervals: 3.41s at priority 150
  behind a one-second master, not a fixed 3.06s.
- **A backup discards a lower-priority advertisement** when preemption is
  enabled, per RFC 5798 §6.4.2: it resets neither the master-down timer nor the
  learned interval. An advertisement with priority zero sets the timer to
  `Skew_Time` instead of a full interval.
- The state machine tracks `Master_Adver_Interval`, learned from accepted
  advertisements, because the takeover delay follows the master rather than local
  configuration.
- One message format serves both address families, with a 4-bit reserved field
  sharing an octet with the interval. The decoder ignores a non-zero reserved
  nibble, as the RFC requires of a receiver.

### Added in this milestone

- `highland-vrrp` is a working codec: `Version`, `PacketType`, `Vrid`,
  `Priority`, `MaxAdverInt`, `IpFamily`, `Advertisement`, `Checksum`, and
  `ChecksumScope`, all validated on construction.
- Two-phase decoding. `Peek::read` validates the fixed header without trusting
  the count, and the addresses are read only after the checksum verifies.
- `Advertisement::encode_v4` and `encode_with_checksum`, plus
  `decode_verified` and `decode`. An IPv6 checksum needs the packet's addresses,
  so the scope is a required argument rather than a guess.
- The RFC 1071 checksum with the RFC 2460 §8.1 pseudo-header, cross-checked
  against a second, naive implementation over many lengths and offsets.
- Six fuzz targets under `fuzz/fuzz_targets/`, run as a 60-second smoke in CI.
- Seven checked-in packet vectors with a stated provenance, and 9 property tests
  including one that flips every bit of a valid packet and requires each
  corruption to be detected or rejected.

### Fixed in this milestone

- The checksum accumulator dropped an odd trailing octet instead of padding it
  with a zero as RFC 1071 requires. Found by the cross-check against a naive
  implementation, not by inspection.
- `MaxAdverInt::from_duration` truncated to milliseconds before converting to
  centiseconds, so 1.5s became 100 centiseconds instead of 150.

### Known limitations

- `Event::ActionSucceeded` and `Event::InterfaceBroughtUp`, and the transition
  reason `preemption_delay_elapsed`.
- 58 state-machine tests, one per invariant, and 14 property tests that assert
  the invariants across randomly generated event sequences.
- `proptest` as a workspace dev-dependency.
- The invariants below are now enforced by tests: `I-01` to `I-04`, `I-09` (the
  generation guard), `I-10` to `I-15`, `I-19` (the state machine's share),
  `I-20`, `I-21`, `I-23` to `I-33`, `I-35` to `I-45`.

### Known limitations

- No VRRP traffic is sent or received. VIP ownership moves only once Milestone 3
  lands.
- `highland run` starts the daemon skeleton and stops; there is no control
  socket listener yet.
- Several errors in `SPEC.md` §18 are aggregated per crate rather than in one
  `HighlandError`, because the CLI and the daemon do not share a dependency set.
  See `docs/architecture.md`.
- No executor applies the machine's actions yet. `highland-net` has the trait but
  no Linux implementation, so nothing adds or removes an address. The socket
  listener arrives with Milestone 7.
- `I-09` is not yet covered: it belongs to the reload planner, in Milestone 4.
  `SPEC.md` §27 says which part of each partly-owned invariant is tested where.
- The packet vectors are encoder-produced, not captured from another
  implementation. Real captures arrive in Milestone 8, and until then the IPv4
  checksum scope rests on the RFC's silence rather than on observed
  interoperability.

## [0.0.0]

- Initial repository.
