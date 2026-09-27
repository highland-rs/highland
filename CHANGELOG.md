# Changelog

All notable changes to Highland are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

Milestones 0 through 3 have landed, and the daemon now carries VRRP over a real
socket: two namespaces on a bridge elect one master and the virtual IP moves when
that node is killed. The control socket, the metrics endpoint, transactional
reload, and gratuitous ARP remain.

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

### The raw VRRP socket

`highland-net` has a real socket at last, so the daemon can carry VRRP:

- A `SOCK_RAW` socket for IP protocol 112, bound to the instance's interface and
  source address, with the TTL set on the socket so the kernel puts 255 in the
  header it builds (RFC 5798 §5.1.1.3).
- `recvmsg` for receives, because a raw socket receives the payload with the IP
  header stripped and the TTL only arrives as ancillary data. The check RFC 5798
  requires cannot be made without it.
- `nix` for that half and `socket2` for the rest, so no `unsafe` is needed in
  this workspace. `pnet_datalink` would have covered it too, but it has been
  unmaintained since May 2024.
- The datagram may arrive with its IP header still attached, which is what
  happens on the loopback path. Both shapes are handled and both are tested,
  because a transport that only assumed one of them would corrupt a valid
  advertisement on the other.
- `bind_with_ttl` exists so a test can produce a packet the receiver must
  reject. The daemon has no reason to send anything but 255.

Six socket tests against a real kernel, behind `netlink-tests`.

Two more bugs only a live socket found, both invisible in the types:

  - `IPV6_RECVHOPLIMIT` was being set on IPv4 sockets, which the kernel
    answers with `ENOPROTOOPT`. Setting both options looks harmless and is not.
    This is also what caused the `ENOPROTOOPT` I had earlier attributed to
    loopback addresses; that attribution was wrong.
  - A raw socket's receive buffer can carry the whole IP packet, not just the
    payload, so the header is parsed and removed rather than assumed absent.

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
- `crates/highland-net/tests/socket.rs`: six tests of the socket against a real
  kernel, and five unit tests of the header handling.
- Seven Netlink tests against a real kernel, behind the `netlink-tests` feature,
  each creating its own dummy interface. They assert `I-19` directly: a
  successful add means the kernel state changed, read back from the kernel.
- `NetlinkBackend::create_dummy` and `remove_dummy`, compiled only with
  `netlink-tests`, because the namespace harness needs them and the daemon does
  not.
- A link and address subscription, which is how a node learns an interface went
  away.

### The daemon runs

- `SocketTransport` binds the socket, sends to every configured peer, and reads
  and validates one datagram without blocking. The rate window is owned by the
  reader rather than the transport, so sending and receiving share one socket
  without a lock between them: a node mid-takeover is still advertising while it
  is listening.
- `VrrpTransport` is the daemon's `Transport` implementation, and a reader task
  turns accepted advertisements into state-machine events. A rejected datagram is
  counted and dropped there, so an unauthenticated peer cannot drive the machine.
- The runner starts one backend, one transport, one reader, and one actor per
  instance, and the daemon refuses to start on a platform with no socket.
- The source address is the interface's primary address, not the virtual address,
  as RFC 5798 §5.1.1.1 requires. A raw socket cannot bind to an address the
  interface does not have yet, and the virtual address is only added on becoming
  master, so binding to it fails with `EADDRNOTAVAIL` on every node.
- Role changes are logged with their reason, and advertisements received with the
  peer that sent them.

### Two namespaces, one segment, and a VIP that moves

`crates/highland-daemon/tests/two_node.rs` builds the topology a VRRP segment
actually is: two namespaces, one bridge, one veth pair each, two daemons. It
asserts that exactly one node takes the address, that killing the master moves it
to the survivor within `Master_Down_Interval`, and that the survivor recorded the
role change that took ownership. This is the `M-04` exit criterion, and the
timing assertion is the point: a failover that works but takes ten seconds is not
a failover.

It found four real defects, none of which any amount of unit testing would have:

  - The equal-priority tie-break was specified in `election` and never called.
    Two nodes that start together both time out and both advertise at the same
    priority, and the master path only stepped down for a *strictly* higher one,
    so both stayed master. The machine now applies the same rule `decide`
    documents, and needs its own primary address to do it, which the runner
    supplies.
  - `Accepted` carried the decoded advertisement but dropped the address it came
    from, so the reader named the local node as the peer. It now carries the
    source.
  - The daemons were started in the container's namespace rather than their own,
    where they saw the container's `eth0` and its address.
  - A present but partial `[logging]` table demanded every key, because a
    field-level `#[serde(default)]` only applies when the whole table is absent.

### Not delivered, and why

- **The control socket.** The message model exists and the CLI can encode it, but
  nothing serves it yet, so `highland status` has no daemon to talk to.
- **Metrics and transactional reload.** `SIGHUP` re-reads and re-validates the
  file and reports a rejection, but the result is not applied to running
  instances, which is most of what "transactional" means.
- **Gratuitous ARP**, which needs an `AF_PACKET` socket whose `sockaddr_ll` has no
  safe representation. It returns `NetError::Unsupported`, and the state machine
  already treats that failure as non-fatal.
- **The namespace harness**, which is Milestone 4's exit criterion and needs root.
  What the scripted-kernel tests establish is that the logic and the sequencing
  are right, so that the namespace run has one variable rather than two.

### Known limitations

- `highland run` exits non-zero: `the VRRP transport is not wired in yet; the
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
