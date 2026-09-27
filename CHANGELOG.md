# Changelog

All notable changes to Highland are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

Milestones 0 through 3 have landed. No virtual IP moves on a real network yet:
the raw VRRP socket is the one missing piece, and the daemon refuses to start
rather than run a process that claims to be a VRRP router while sending nothing.

### Added — Milestone 0, the repository

- `docs/SPEC.md`: the normative specification, with scope tiers (`[I]` initial
  release, `[1]` version 1.0, `[F]` post-1.0), RFC 2119 language, and stable
  requirement identifiers (`G`, `R`, `I`, `V`, `L`, `S`, `M`, `D`).
- A nine-crate workspace matching the dependency rules in `SPEC.md` §9.
- The typed domain vocabulary: `Role`, `Event`, `Action`, `TimerId`,
  `Generation`, `TransitionReason`, plus the `Clock` and `Rng` abstractions with a
  deterministic `ManualClock`.
- `highland-config`: the typed configuration model, a strict TOML parser with
  explicit-unit durations, and the `V-01` through `V-32` validation rules, each
  with its own test.
- `highland-observe`: the event model, the redaction layer, and a bounded event
  ring.
- `highland-control`: the control request and response messages, the error
  taxonomy, and a token-bucket rate limiter.
- Continuous integration covering formatting, lints, tests, dependency policy,
  advisories, the MSRV, and documentation.
- End-user documentation under `docs/user/`, plus architecture, testing, threat
  model, compatibility, and three ADRs.

### Added — Milestone 1, the pure state machine

- `timer` (a `TimerSet` and a `RetryPolicy`), `health` (the policy arithmetic of
  `SPEC.md` §12.2), `election` (the four-step tie-break of §12.3), and `machine`
  (the full `InstanceStateMachine`).
- 58 state-machine tests, one per invariant, and 14 property tests asserting the
  invariants across randomly generated event sequences.
- `proptest` as a workspace dev-dependency.
- The invariants now enforced by tests: `I-01` to `I-04`, `I-09` (the generation
  guard), `I-10` to `I-15`, `I-19` (the state machine's share), `I-20`, `I-21`,
  `I-23` to `I-33`, `I-35` to `I-45`.
- `Event::ActionSucceeded`, `Event::InterfaceBroughtUp`, and the transition
  reason `preemption_delay_elapsed`.

### Changed — Milestone 1

- Ownership is a two-phase handshake: the machine requests addresses and enters
  `MASTER` only on `Event::ActionSucceeded`. There is no path from `BACKUP` to
  an advertisement without confirmed ownership, which makes `I-04` structural
  rather than a matter of action ordering.
- A failed address removal keeps the instance `MASTER` with advertising stopped,
  because the addresses are still present. The role changes once removal is
  confirmed.
- A shutdown or a pause cancels an in-flight ownership request and owes a
  best-effort removal, so a late confirmation cannot revive an instance.

### Added — Milestone 2, the VRRPv3 codec

- A working codec: `Version`, `PacketType`, `Vrid`, `Priority`, `MaxAdverInt`,
  `IpFamily`, `Advertisement`, `Checksum`, and `ChecksumScope`, all validated on
  construction.
- `Advertisement::encode_v4` and `encode_with_checksum`, plus `decode_verified`
  and `decode`. An IPv6 checksum needs the packet's addresses, so the scope is a
  required argument rather than a guess.
- Two-phase decoding: `Peek::read` validates the fixed header without trusting
  the count, and the addresses are read only after the checksum verifies.
- The RFC 1071 checksum with the RFC 2460 §8.1 pseudo-header, cross-checked
  against a second, naive implementation over many lengths and offsets.
- Six fuzz targets under `fuzz/fuzz_targets/`, run as a 60-second smoke in CI.
- Seven checked-in packet vectors with a stated provenance, and 9 property tests
  including one that flips every bit of a valid packet and requires each
  corruption to be detected or rejected.

### Fixed — Milestone 2

- The checksum accumulator dropped an odd trailing octet instead of padding it
  with a zero as RFC 1071 requires. Found by the cross-check against a naive
  implementation, not by inspection.
- `MaxAdverInt::from_duration` truncated to milliseconds before converting to
  centiseconds, so 1.5s became 100 centiseconds instead of 150.

### Added — Milestone 3, the Linux backend and the executor

- A real Linux netlink backend: interface lookup with addresses, link state,
  address add and remove **confirmed by read-back** rather than by acknowledgment
  (`I-19`), and a link/address subscription.
- `highland-net::vrrp`: the receiver-side rules, in the order a receiver should
  apply them. The TTL must be 255, the source must be a configured peer, the VRID
  must match, and the length must agree with the count before anything is
  allocated from it. 18 unprivileged tests.
- `highland-net::ScriptedBackend`: a scriptable kernel, which is what `R-03`
  asks for and what makes the failure paths testable without privileges.
- `highland-daemon::Executor`, which applies one action and answers with the
  outcome, and a `Transport` trait, so the wire can be swapped without touching
  the failover logic.
- `highland-daemon::InstanceActor` and `run_instance`: one task per instance, no
  shared state, one timer for the earliest deadline rather than six tasks.
- 15 daemon tests, including a whole failover driven end to end: startup,
  takeover, ownership confirmed before advertising, a failed add faulting instead
  of claiming the address, relinquishment, shutdown, and a paused instance
  staying out.

### Changed — Milestone 3

- `NetworkBackend` is now `async`. Netlink is an asynchronous socket, and a
  synchronous trait would have forced a blocking wrapper on the runtime thread,
  which is exactly what `I-38` forbids. A scripted backend is unaffected: its
  futures are already complete.
- The action loop is a **worklist**, not a single pass. Confirming the addresses
  is what makes the machine enter `MASTER` and ask to advertise, so the actions
  produced by an outcome must be applied too. A single pass left a master that
  owned its address and never said so; the failover test caught it.

### Corrected — Milestone 3

- **The advertisement interval is a 12-bit centisecond field** (RFC 5798
  §5.2.7), not the 8-bit field the specification assumed, so the accepted range
  is 10ms to 40.95s. `V-04` and the configuration bound were both wrong; the
  error message still quotes the old 2550ms bound.
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

### Fixed, by running it on Linux

The Netlink backend had never been compiled, because it is `cfg`'d out on
macOS. Compiling it in a container found **thirteen errors** and then two real
bugs that no amount of reading would have found:

- The `Connection` future that drives the Netlink socket was being dropped
  instead of spawned. Every request then failed with "not acknowledged", while
  every type still lined up. Only a live socket shows this.
- A by-name interface lookup that matches nothing arrives as `ERANGE`, not as an
  empty dump, so "no such interface" was surfacing as a transport error.

Also corrected: the `rtnetlink` feature is `tokio_socket`, not `tokio`, and
`Handle` takes no type parameter. Both were written from a wrong assumption.

### Added

- `scripts/linux-tests.sh`: the whole gate in a container with `CAP_NET_ADMIN`,
  which is how the Linux-only code gets compiled and executed from a Mac.
- Seven Netlink tests against a real kernel, behind the `netlink-tests` feature,
  each creating its own dummy interface. They assert `I-19` directly: a
  successful add means the kernel state changed, read back from the kernel.
- `NetlinkBackend::create_dummy` and `remove_dummy`, compiled only with
  `netlink-tests`, because the namespace harness needs them and the daemon does
  not.
- A link and address subscription, which is how a node learns an interface went
  away.

### Not delivered, and why

- **The raw VRRP socket.** It needs `SOCK_RAW` for IP protocol 112, and reading a
  peer's TTL back needs ancillary data. Both are Linux-specific, and this
  milestone was built on macOS, where neither can be compiled or verified. The
  daemon therefore refuses to start (`TRANSPORT_AVAILABLE` is `false`).
- **Gratuitous ARP**, which needs an `AF_PACKET` socket whose `sockaddr_ll` has no
  safe representation. It returns `NetError::Unsupported`, and the state machine
  already treats that failure as non-fatal.
- **The namespace harness**, which is Milestone 4's exit criterion and needs root.
  What the scripted-kernel tests establish is that the logic and the sequencing
  are right, so that the namespace run has one variable rather than two.

### Known limitations

- `highland run` exits non-zero: `the VRRP transport is not implemented; the
  daemon will not start`.
- No VRRP traffic is sent or received, so no address moves between machines.
- No control socket listener, so every `highland` command except `version` and
  `check-config` fails to connect.
- `SIGHUP` re-reads and re-validates the file and reports the outcome, but does
  not apply it: the reload planner and `I-09` arrive with Milestone 4.
- No health probes and no check scheduler, so a failing check cannot demote a
  node.
- No metrics endpoint, and no `tracing` bridge in `highland-observe`; `Action::Log`
  and `Action::EmitEvent` are discarded.
- `--enable-force-transition` and the `--yes` confirmation prompts are not wired
  up.
- The shutdown budget is declared but not timed, and no VIP is relinquished on
  shutdown, because the daemon never starts.
- The netlink tests need `CAP_NET_ADMIN` and are behind the `netlink-tests`
  feature; they are not in CI. The `netns` CI job is still `if: false`.
- The packet vectors are encoder-produced, not captured from another
  implementation. Real captures arrive in Milestone 8, and until then the IPv4
  checksum scope rests on the RFC's silence rather than on observed
  interoperability.
- Several errors in `SPEC.md` §18 are aggregated per crate rather than in one
  `HighlandError`, because the CLI and the daemon do not share a dependency set.
  See `docs/architecture.md`.

## [0.0.0]

- Initial repository.
