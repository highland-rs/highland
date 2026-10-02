# Changelog

All notable changes to Highland are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed

- **An oversized datagram was silently cut short and then validated.** `receive`
  read `outcome.bytes`, the number of bytes the kernel copied into the buffer,
  and never read `outcome.flags`. On Linux that count never exceeds the buffer,
  so the `min` against `buffer.len()` could not clip anything and `MSG_TRUNC` was
  the only signal that the datagram had been truncated — a signal nothing was
  reading. A peer that sent more than the receive buffer got its first 4096 bytes
  handed to `strip_header` and then to `validate`, where the checksum in the
  datagram covers bytes the receiver never saw. Both family arms now refuse a
  truncated datagram with `NetError::TruncatedDatagram`, which is the read side
  of `L-10`.

## [0.2.1] - 2026-09-30

Three defects, all in the ownership path, all found by auditing the product
against its own specification rather than by testing what already worked.

Every one of the three is a way for a node to believe it is master while it is
not — or while a peer is. Two produce a split brain, the condition `I-14` exists
to prevent, and the third leaves a master advertising into a black hole. There
are no behaviour changes for an operator to plan around and no API breaks, which
is why this is 0.2.1 rather than 0.3.0: an operator who does nothing is no worse
off than on 0.2.0, and an operator who runs `force-transition`, stops the daemon
under load, or loses the ability to send advertisements is better off.

Two of the three were found because the code did not do what the rule beside it
said. A window that could never fire and a shutdown path that documented a
budget it never enforced are the same failure: the specification was right, the
implementation was silent about the difference, and nothing tested the gap.

### Fixed

- **A forced return to `INIT` released nothing** (`I-14`). The `Role::Init` arm of
  `force_transition` set `owns_addresses = false` and cleared `pending` on its
  own, and never asked the executor to remove the addresses. `highland
  force-transition --role init --confirm` on a master therefore cancelled the
  advertisement timer and left the virtual address on the interface. The node
  went silent but kept forwarding for the VIP. A peer that then timed out on its
  master-down timer added the same address, and both nodes believed they owned
  it — the split brain `I-14` exists to prevent. Every other role arm already
  released through `request_release`; this one now does too, and a forced `INIT`
  during an in-flight acquisition still owes the best-effort removal that
  `abandon_acquisition` performs elsewhere.


- **A master that could not advertise stayed master** (`I-24`).
  `on_advertisement_failure` anchored its failure window on the *first* failure
  of a streak and never moved it, then required the current time to fall inside
  that window to fault. Once the opening failure aged past
  `ADVERTISE_FAILURE_WINDOW` — ten seconds — the condition was permanently false.
  A node whose first advertisement send failed and then went on failing
  intermittently incremented a counter that could no longer trigger anything: it
  kept `MASTER`, kept the virtual address, and kept advertising into a black hole
  while peers that had timed out contested it. The limit is now counted over
  consecutive failures, which the streak already resets on any success, a role
  change, or an ownership confirmation. The constant's own doc comment said
  failures were counted "within" the window, so the code did not match its
  documented intent.

- **The daemon could exit before it had given the address back.** On `SIGTERM` the
  run loop sent the shutdown signal, built a `ShutdownPlan`, logged each step of
  the daemon's own sequence, and returned — the instance `JoinHandle`s were
  dropped without a single `await`, and `DEFAULT_SHUTDOWN_BUDGET` was defined and
  enforced nowhere. Each actor still had to observe the signal, send its
  priority-0 advertisement, and have the executor's `RemoveVirtualAddresses`
  netlink call return. The process could exit mid-handshake, leaving the
  relinquishing node's kernel answering ARP for a VIP the peer had already taken
  over after waiting `Skew_Time`. The handles are now joined within the plan's
  own budget, and a task that overruns it or has panicked is reported rather than
  dropped in silence.

### Testing

The first and second fixes are covered by tests that fail against 0.2.0. The
third is not: `run` needs a live signal, a valid configuration, and a real
backend, and the handles are not reachable from a test. The ordering it restores
is covered at the machine level by `I-14` and `I-32`, but nothing yet verifies
end to end that a real master removed its address before the process went away.
That gap is recorded rather than papered over.


## [0.2.0] - 2026-09-29

Seven defects, all found by auditing the product against its own specification
rather than by testing what already worked. Four of them changed behaviour in a
way an operator can observe, which is why this is 0.2.0 and not 0.1.1: a
configuration that started cleanly on 0.1.0 can now be refused, and a reload
that reported success can now be refused instead.

Nothing here is an API break, and nothing here was found by a fuzzer. The fuzzer
found nothing; it is worth saying plainly that a clean fuzz campaign is weak
evidence, and that what actually paid was asking the running binary to reject
things the documentation said it rejected.

### Behaviour changes

An operator upgrading from 0.1.0 should read these four.

- **An instance may no longer peer with this node** (`V-08`). The rule was
  documented, unit-tested, and enforced nowhere: `Options.local_addresses` was
  documented as "used to reject self-peering", defaulted to an empty list, and was
  never read. A node could name its own address as a peer, `check-config` would
  call the file valid, the daemon would start, and the node would unicast its own
  advertisements to itself. Both the startup path and the reload path now supply
  the host's addresses, and the daemon refuses to start if they cannot be read —
  "unknown" is not "none".
- **`check-config` validates against the host it runs on.** It used to answer
  "unknown" for everything a document cannot say, so it could not catch `V-08` or
  `V-22` at all. A missing interface is now refused up front, naming the rule and
  the way out (`defer_interface_binding`), instead of after loading the
  configuration and reaching the bind with a raw netlink error.
- **An instance whose health check cannot run no longer takes its VIP.** A check
  type this build does not implement — `https`, `dns`, `process`, `file`,
  `composite`, `command` — was logged as an error and the instance started
  anyway, then elected itself master and reported itself `healthy`, with the check
  that was supposed to hold it down never having run. The instance is now refused
  before it is created. The node keeps running and other instances keep
  participating, because one unusable instance is not a reason to stop
  administering the rest.
- **A reload that changes the peer list, the multicast settings, or
  `allow_unconforming_hop_limit` is now refused.** None of the three was compared
  by the reload planner, so a change to any of them classified as "no change": the
  reload reported `Applied`, the generation advanced, the event history recorded
  `reload_accepted`, and the node carried on using the values it had started with.
  The third is the sharpest — it is the switch that relaxes TTL and hop-limit
  enforcement, so a file saying it is off while the running socket had it on was a
  security-relevant divergence that nothing reported.

### Fixes

- **The IPv6 Neighbour Advertisement was malformed and was discarded by the
  kernel.** `gratuitous::neighbour_advertisement` built a 30-octet message against
  the 32 it reserves: the flags field was one octet where RFC 4861 §4.4 has four,
  and the option length was two octets where it has one. On an IPv6 takeover the
  announcement was sent and dropped, the neighbour cache kept pointing at the dead
  node, and traffic blackholed until that entry aged out. IPv4 is unaffected;
  gratuitous ARP is a separate encoder.
- **A reload produced no audit event and named no peer.** `SPEC.md` §22.1 lists
  `reload` as destructive, so `R-28` requires an audit event naming the peer
  credential. `EventName::ReloadAccepted` and `EventName::ReloadRejected` existed
  for exactly this and had zero uses. The outcome *was* in the history, under
  `EventName::DaemonLifecycle`, with `reload_accepted` as its reason — so an
  operator filtering by event name could not find it. It is now its own event,
  naming the peer, recorded where both entry points already meet so a `SIGHUP` and
  a control-socket reload cannot disagree.
- **The event history lost events silently.** The ring is bounded at 4096 and has
  always known how many it dropped; `dropped()` was called from tests only. A
  follower that was away long enough received a history with a hole in it and no
  way to know. A bounded history now reports how many events before the client's
  cursor are gone.

### Testing

- **A new fuzz target for the encoder.** All six existing targets called `decode`;
  nothing called the encoder. Producing bytes that `decode_verified` accepts needs a
  correct version, type, VRID, a count that agrees with the length, and a verifying
  checksum — 307 million executions of `fuzz_vrrp_ipv4_packet` never entered it.
  `fuzz_vrrp_round_trip` constructs a valid advertisement from arbitrary fields
  and asserts that encode-then-decode preserves every field, that re-encoding is
  the identity, and that the checksum depends on the RFC 2460 pseudo-header. The
  last property guards a defect this project shipped once and that no decode-side
  check can see.
- **The minimized corpus is checked in.** It is 19MB on disk and about 580KB in
  the repository, so the original "too large to commit" objection held of the
  working tree and not of the repo. CI's 60 seconds now starts from real coverage
  and gets stronger over time instead of resetting every run; the packet targets
  went from 29 inputs to 10 million executions in 20 seconds.
- **`scripts/audit-rules.py`** asks a real binary to reject one configuration per
  documented rule. It found `V-08` and `V-22` in the published 0.1.0 binary, and is
  the regression net for both.
- **A two-node failover budget was 186ms tight.** `Master_Down_Interval` for a
  1s interval at priority 150 is 3.414s by RFC 5798 §6.1, and the test allowed
  3.6s — so a loaded runner failed a test whose assertion blamed the protocol code
  for the test's own arithmetic. The budget is now derived from `skew_time`.

### Known gaps

Stated here rather than left to be found:

- **There is still no test that proves a kernel accepts Highland's Neighbour
  Advertisement and moves a real neighbour cache.** The IPv4 twin of that test
  exists; the IPv6 one does not, and that gap is where the malformed message lived.
  The fix is verified against RFC 4861 §4.4 byte by byte, not against a kernel.
- **`check-config` still reports an unimplemented check type as valid.** The
  refusal is at run time, where the instance is stopped, because refusing in
  validation stops the whole daemon and one bad instance should not take down the
  others.
- **CI was, for one run, green in the only sense available: nobody had pressed the
  button.** The `ci` workflow had two runs in its history before this release, and
  three separate failures in the first of them were in jobs that had never
  executed.


## [0.1.0] - 2026-09-28

The first public release. Milestones 0 through 6 have landed, which is more than
0.1.0's scope asked for: the milestone plan placed IPv6, multicast, and health
checks after it, and they are here because they were needed to interoperate and
because leaving them out would have meant shipping a VRRP implementation that
only spoke to itself.

Nothing in this release is a stable API. `SPEC.md` §26 permits API changes before
1.0; configuration changes get a note here; protocol behaviour is
standards-compliant and tested against Keepalived.

What is *not* in it, and is on the landing page rather than buried here: a node
that is killed, frozen, or partitioned keeps the address it holds, because nothing
can tell it otherwise. See the [split-brain notes](docs/user/compatibility.md).

### Release procedure

`publish.sh` does the publishing, and it is tracked in the repository because a
procedure that lives in one person's shell history is a procedure the next
person retypes from memory:

```console
$ ./publish.sh --dry-run   # prints the order, and what is already published
$ ./publish.sh
```

The order is the dependency graph, not the alphabet and not the directory order.
It is written into the script, and `--dry-run` re-derives it, so a crate added
without a case fails visibly instead of publishing into a hole.

Two things about publishing that cost time on 0.1.0 and are now handled:

- **The script waits for the index rather than sleeping.** crates.io propagates a
  new release asynchronously, and publishing a dependent before the dependency is
  visible fails with a "no matching package" error that reads like a broken
  version requirement and invites the expensive wrong response of bumping a
  version. The first 0.1.0 attempt stopped at `highland-checks` for exactly this
  reason.
- **A partial run is resumable.** Each crate is checked against the registry and
  skipped if already published, so finishing a failed run is a re-run rather than
  a re-derivation of what is left.

After publishing, confirm from outside the workspace — a registry-only consumer
proves the packaging, which is where the symlinked licences and readmes either
worked or did not:

```console
$ cargo install highland-cli --version 0.2.0
$ highland version
highland 0.2.0
```

### Historical notes

Milestones 0 through 4 first landed together, when the daemon carried VRRP over a
real socket, answered operators over a control socket, exported Prometheus
metrics, applied a transactional reload without dropping the address it held,
recorded an event history, and announced a takeover to the segment. Two namespaces
on a bridge elect one master, the virtual IP moves when that node is killed, and a
third namespace that runs no daemon sees its neighbour cache entry change with it.

A segment with faults on it is now part of the test suite: loss, reordering,
duplication, a one-way partition, a link flap, and a frozen process, each
asserted for bounded recovery and bounded event volume. That suite found five
defects, listed below.

Milestone 4 is complete. What remains is Milestone 5, IPv6 and multicast.

#### Milestone 0, the repository

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

#### Milestone 1, the pure state machine

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

#### Milestone 1, changed

- Ownership is a two-phase handshake: the machine requests addresses and enters
  `MASTER` only on `Event::ActionSucceeded`. There is no path from `BACKUP` to
  an advertisement without confirmed ownership, which makes `I-04` structural
  rather than a matter of action ordering.
- A failed address removal keeps the instance `MASTER` with advertising stopped,
  because the addresses are still present. The role changes once removal is
  confirmed.
- A shutdown or a pause cancels an in-flight ownership request and owes a
  best-effort removal, so a late confirmation cannot revive an instance.

#### Milestone 2, the VRRPv3 codec

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

#### Milestone 2, fixed

- The checksum accumulator dropped an odd trailing octet instead of padding it
  with a zero as RFC 1071 requires. Found by the cross-check against a naive
  implementation, not by inspection.
- `MaxAdverInt::from_duration` truncated to milliseconds before converting to
  centiseconds, so 1.5s became 100 centiseconds instead of 150.

#### Milestone 3, the Linux backend and the executor

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

#### Milestone 3, changed

- `NetworkBackend` is now `async`. Netlink is an asynchronous socket, and a
  synchronous trait would have forced a blocking wrapper on the runtime thread,
  which is exactly what `I-38` forbids. A scripted backend is unaffected: its
  futures are already complete.
- The action loop is a **worklist**, not a single pass. Confirming the addresses
  is what makes the machine enter `MASTER` and ask to advertise, so the actions
  produced by an outcome must be applied too. A single pass left a master that
  owned its address and never said so; the failover test caught it.

#### Milestone 3, corrections

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

#### Milestone 3, fixed by running it on Linux

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

### The control socket

`highland status` is the first thing an operator runs, and until now nothing
proved it worked. It does.

- `highland-control` serves a Unix socket speaking newline-delimited JSON, with a
  `Service` trait so the server has no opinion about VRRP. The daemon supplies the
  behavior; the crate supplies authenticating, rate limiting, and framing.
- The socket is created with mode `0660` and a world-writable socket is refused
  at startup rather than served. A test asserts the mode on a real socket, and
  another asserts the refusal.
- Each peer is rate limited, an oversized request is refused before it is parsed,
  a malformed request gets a typed refusal instead of a dropped connection, and a
  client that connects and says nothing is timed out rather than holding a task
  forever. Each has a test over a real socket.
- Operator actions are audit records naming the peer. `force-transition` is
  refused unless the daemon was started to allow it, and refuses again without an
  explicit confirmation.
- A status registry publishes each instance's role, priorities, addresses, and
  last reason after every event, so the control API and the metrics can never
  disagree with the state machine. An actor owns its machine, and a reader cannot
  ask an actor what its role is; publishing is what closes that gap.
- The daemon serves the socket after its instances exist, so the first `status`
  already describes the real thing. A failure to bind is a warning, not a refusal
  to start: an operator who never calls `highland status` should not lose
  forwarding.

Ten tests drive a real socket with a real client, and two more run the real CLI
against a real daemon in a namespace: one asks it what it is doing, the other
makes a master give up its address and checks the kernel released it.

Three defects came out of writing them:

  - The reader's channel had its receiver dropped on the line after it was
    created, so the reader exited immediately and no advertisement was ever
    delivered. The two-node test caught it as split brain, and the fix is why each
    instance now has two channels: one for the wire, one for the operator, so a
    flood of advertisements cannot delay a `relinquish`.
  - A refusal from the daemon still exited zero, so a script running
    `highland show nope && deploy` would have gone on to deploy. A refusal is now
    a non-zero exit, printed as well as returned.
  - A Unix socket path is capped at 108 bytes and an interface name at 15, and
    both were discovered by a fixture being rejected with a confusing error. The
    socket limit is now checked up front with a sentence an operator can act on,
    and the test fixtures use names inside the limits.

### The metrics endpoint

A scraper can read what the daemon is doing, and cannot be told something the
control API would disagree with.

- `highland-observe` gained the metric primitives: counters, gauges, histograms,
  and a Prometheus text renderer. It depends on nothing but `serde` and
  `thiserror`, because a metrics library that drags in a web framework is a
  metrics library that cannot be tested.
- Cardinality is a property of the type, not of discipline: label values are
  fixed when a series is declared, and a rejection reason outside the known set
  is folded into `other`. A test asserts that a peer address fed in as a reason
  never appears in a label (`R-20`, `L-10`).
- A metric that has never fired still appears, as zero. A dashboard that breaks
  when a node has nothing wrong with it is a dashboard that breaks on the wrong
  day.
- The endpoint is four lines of HTTP rather than a framework, it answers `GET`,
  refuses anything else, states its `Content-Length` so a scraper knows when the
  body ends, and times out a client that connects and says nothing.
- Both views come from one status registry, so the role in a scrape and the role
  from `highland status` are the same fact read twice. The test asserts exactly
  that, and it is the reason the control API had to be built first.
- Metrics are bound to the node's address, not to loopback, and the test scrapes
  from inside the node's namespace: a namespace has its own loopback and its own
  routes, so a scrape from the test's namespace would be testing a different
  network stack.

**Only series with a producer are exported.** `highland_check_failures_total` and
`highland_check_duration_seconds` are named in `SPEC.md` §16.2 but have nothing
producing them until the checks land in Milestone 6, and a metric that is always
zero is a lie about the system. They are absent on purpose, and a test asserts
they are.

A poisoned metrics lock is recovered from rather than propagated: a panic in a
counter must not stop a node that owns a VIP.

### Transactional reload

A reload is a transaction, and both halves of that are now tested against a
running node.

- `highland_daemon::reload::plan` compares the running configuration with a
  candidate and classifies every instance as unchanged, reloadable, added, or
  needing a restart. It is a pure function, so it is tested exhaustively without
  a kernel, a socket, or a daemon.
- **A reload is applied only if no instance needs a restart.** A partial reload
  that leaves one instance on old settings is the state the word
  "transactional" exists to prevent (`I-09`, `R-46`), and a refusal names the
  instance and the change.
- `advertisement_interval` is a restart, not a reload: the running advertisement
  timer was armed from the old value, so changing it underneath a master would
  leave the timer firing at the old rate.
- A reloadable instance is reconfigured **in place**: the role, the ownership,
  and the armed timers all survive. A reload is not a restart (`R-48`), and the
  test asserts the address does not move.
- Applying a reload bumps the generation, so a result still carrying the old one
  is discarded (`I-12`, `R-47`), and a reload already out of order is refused.
- The running configuration is replaced, so the next reload compares against
  what is actually running rather than against the file the daemon started from.

### A segment with faults on it

`crates/highland-daemon/tests/chaos.rs` injects faults with `tc netem` on one
node's own egress — never on the shared bridge, because the question is what one
node does to its peer on its own. Five scenarios: loss, a one-way partition,
reordering and duplication, a link flap, and a frozen process. Each asserts
bounded recovery inside the timing budget and bounded event volume, because a
daemon that recovers by flapping has not recovered: every flap is another
address move for every client.

Two of the five assert what a VRRP implementation *cannot* fix, on purpose. A
frozen or partitioned node still believes it is master and still holds the
address, because no protocol message reaches it. The assertions are about what
happens when the fault heals, which is the same fencing limitation the failover
suite records for a killed node.

### Fixed, by running it on a segment with faults

- **A master advertised once and then went silent.** The advertisement timer was
  armed at takeover and never re-armed. Nothing noticed, because one
  advertisement is enough to keep resetting a backup's `Master_Down_Interval`;
  under loss it is not, and the address moved on every interval. Now `I-47`.
- **Nothing watched the interface.** The machine has handled `InterfaceDown` and
  `InterfaceUp` since Milestone 1 and no producer existed, so a pulled cable was
  discovered by an address operation failing: the node held the address on a
  dead link for about three seconds and then went to `FAULT` rather than
  relinquishing. Each instance now polls the kernel and reports the transition.
  Now `I-48`.
- **A faulted instance seized the address on a retry backoff.** A node returning
  from a link flap grabbed the address from a live master. The retry now tells
  *away* from *unable*: a local failure still re-attempts, which is what the
  hold-down bounds, and a fault caused by the interface going away returns to the
  election and listens for a full `Master_Down_Interval` first.
- **The machine's log and event actions were discarded.** The executor treats
  them as bookkeeping and nothing else looked at them, so a failed announcement,
  a refused takeover, and a rejected action all happened silently, which `R-24`
  forbids.
- **`ScriptedBackend` shared one script across every operation**, so a failure
  scripted for an address add could be eaten by an interface poll. Interface
  lookups have their own queue.

### A takeover that tells the segment

A node that takes over a virtual address now announces it: a gratuitous ARP for
IPv4, written as a complete Ethernet frame on an `AF_PACKET` socket, and an
unsolicited Neighbor Advertisement for IPv6 with the override flag set. Without
it every neighbour keeps a cache entry pointing at the node that had the address
last, and traffic to the VIP is a black hole until that entry ages out.

The proof is the effect rather than the send. A capture on the sending node
proves nothing, because a bridge does not reflect a broadcast back to the port it
came from, so the test adds a third namespace that runs no daemon, resolves the
address to the old master, kills the master, and requires the neighbour's cache to
point at the new one within three seconds. Checked against a build with the
announcement disabled, it fails.

This is the only code in the workspace that uses `unsafe`, in two functions with
documented safety arguments, and `docs/adr/ADR-0004-gratuitous-arp.md` records
why a datalink crate was the wrong answer for one 42-byte frame.

### The event history

`highland events` reports what the instance did as it happened, with the reason,
a sequence number, and a real timestamp. The log is bounded at 4096 entries, and
`--follow` polls with a cursor rather than holding a connection open, so a client
that disappears mid-stream leaves nothing waiting on a socket.

### A reload an operator can actually ask for

`highland reload` and `SIGHUP` are now the same operation: one `ReloadHandle`,
shared by the signal loop and the control API, so they cannot disagree about what
a reload does. A reloadable instance keeps its role, its address, and its timers;
a reload that needs a restart is refused whole, naming the instance and the
field.

### IPv6 and multicast

Four combinations, each two namespaces on a bridge in
`crates/highland-daemon/tests/two_node.rs`: IPv4 unicast, IPv4 multicast, IPv6
unicast, IPv6 multicast. Each asserts that the group was joined — read from the
kernel, because a configuration that says `multicast` and a daemon that never
joined the group look identical in the log — that exactly one node holds the
address, and that killing the master moves it inside `Master_Down_Interval`. This
is the exit criterion of `M-06`, and it meets the IPv6 half of `G-01`.

Writing those tests found four defects, none of which unit tests could see,
because none of them is about logic:

- **A raw IPv4 socket bound to a unicast address cannot receive multicast.**
  The kernel matches the datagram's destination against the address the socket is
  bound to, so a socket bound to `192.0.2.11` never sees a datagram addressed to
  `224.0.0.18`. A multicast instance sent advertisements nobody could receive,
  and failed over on a schedule instead of an election. A group socket now binds
  to any address, and the source is chosen by the outgoing interface.
- **The IPv6 hop limit was read from the wrong ancillary message.** The message
  type is `IPV6_HOPLIMIT`; the socket option that switches the ancillary data on is
  `IPV6_RECVHOPLIMIT`, a different number. Looking for the option number found
  nothing, the hop limit read as zero, and every IPv6 advertisement was rejected
  for a TTL that was correct. IPv4's `IP_TTL` arrives untyped and the same reader
  is right for it, so the reader was right for one family and silently wrong for
  the other.
- **A tentative IPv6 VIP could not be used for about a second.** A tentative
  address cannot be bound to and cannot be a source, so the node owned an address
  it could neither send from nor announce. IPv6 VIPs are now added with
  `IFA_F_NODAD`: the address has just been claimed by an election, so there is
  nothing left to detect.
- **The multicast TTL was never set.** `IP_MULTICAST_TTL` and
  `IPV6_MULTICAST_HOPS` are different socket options from the unicast TTL, and
  both default to **1**. A VRRP advertisement sent with a hop limit of 1 is
  discarded by every receiver, and the node is never heard of again. This is the
  single most likely way to get multicast mode subtly wrong, and it is invisible
  until the nodes stop agreeing.

The multicast socket also names its outgoing interface, because a group is local
to one link by definition and a routing table is neither guaranteed to have an
entry for it nor the right answer when it does.

### Received destinations, and a narrower multicast

`IP_PKTINFO` and `IPV6_PKTINFO` are now read on both families, so a datagram
carries the address it was sent to. Two things need it and neither could do
without guessing:

- an IPv6 checksum covers the destination, so the receiver verifies against what
  the packet actually arrived at rather than what it assumes;
- a group socket on IPv4 is bound to any address, which means it also sees
  datagrams addressed to the host itself, so a multicast instance now checks that
  a datagram really arrived at the group and rejects one addressed to the host.

Advertisements are also built per destination for the first reason. Both ends had
been guessing: the sender always used the default group even when sending to a
unicast peer, and the receiver always used the default group even when the
datagram arrived at its own address. Every IPv6 unicast packet would have failed
its checksum on arrival.

### The configuration surface

`multicast.group` is optional, because the default is per family
(`224.0.0.18` and `ff02::12`) and a single default would be the wrong family for
half the instances — a node that silently joined the wrong group would look
healthy while hearing nothing. A configured group of the wrong family is refused,
as is one that is not a multicast address. A multicast instance needs no peer
list. A mixed-family *peer* list is legal, and a test says so, because it is a
different thing from a mixed-family address list and used to be confused with it.

### Health checks that run

The check model, the debounce thresholds, and the policy arithmetic have existed
since Milestone 1; what did not exist was anything that ran a probe. Now there
are four — `tcp`, `http`, `unix`, and `interface` — a scheduler that bounds and
debounces them, and one task per instance that drives it.

The probes have no new dependency. `http` is a request line, a status line, and
a bounded read, because a health check that reads an unbounded body from a
misbehaving service is how a monitor becomes an amplifier. `interface` reads the
link through a `LinkProbe` trait, so a check and the daemon share one Netlink
socket and cannot disagree about whether a link is up — and so its three failure
modes are tested without three namespaces.

`https` is refused rather than downgraded. A TLS handshake is not something to
reimplement, and a check that connected to port 443 without validating a
certificate would report a service as healthy on the strength of a plaintext
exchange. It is refused *by name*, at startup, with the reason — a check that
cannot be built is a configuration error, and a check that fails on every
interval forever is a quieter way to take a node out of service than a refusal.

The scheduler owns the three things a probe must not get wrong: the timeout, the
thresholds, and staleness. A probe that hangs leaves the instance believing a
verdict that stopped being true, so the bound is the scheduler's and a timeout
counts as a failure. A blip must not move a priority on a live segment, so the
thresholds are not advisory. And a result from an older generation, or one that
finished after a newer result, never overwrites it (`I-26`).

The task sits beside the actor, not inside it: a probe waits on a socket, and a
state machine that waited on a socket would stop deciding anything while a service
was slow (`I-38`, `R-05`).

### Two more defects, both in the reload path

- **A health change on reload was accepted and then discarded.** The reload
  planner classifies `health` as reloadable, and the actor's reconfigure rebuilt
  the machine with the *default* health policy — so the change was reported as
  applied and the old policy stayed. The policy is carried across now.
- **A change to the check list was classified reloadable and could not be.** The
  running scheduler holds probes built from the old list, so it is classified as
  needing a restart instead. Saying "restart" is the honest answer; saying
  "applied" and leaving the old probes in place would be a reload that did
  nothing.

### A note on `preempt`

A demoted master does not give the address up by itself. RFC 5798 has it keep
advertising, and a backup only takes over from a live master when preemption is
enabled — so a deployment that wants a failing check to move the address has to
set `preempt = true` on the peer. The namespace test says so explicitly, because
"my check failed and nothing happened" is the most likely way to be surprised by
this.

### Interoperability, and a protocol defect it found

`crates/highland-daemon/tests/interop.rs` runs Highland and Keepalived 2.3.3 in two
namespaces on a bridge, in both directions: Highland master with a Keepalived
backup, and Keepalived master with a Highland backup that takes the address over
when the master is killed. The suite needs the `keepalived` binary and reports a
skip without it; the Linux gate and a CI job install it.

Before this, Highland had only ever spoken to itself. The first run produced two
masters and a full subnet's worth of silence between them, and the diagnosis took
a `KEEPALIVED_V4_ADVERTISEMENT` constant, a frame capture, and the RFC:

**The IPv4 checksum was wrong.** RFC 5798 §5.2.8 requires the checksum to cover
"the entire VRRP message ... and a 'pseudo-header' as defined in Section 8.1 of
[RFC2460]. The next header field in the 'pseudo-header' should be set to 112
(decimal) for VRRP" — for both families, with no carve-out for IPv4. Highland had
read the IPv4 header's own checksum as a reason to skip the pseudo-header, and
`SPEC.md` A-43 recorded that reading as a decision to be settled later.

Keepalived computes the pseudo-header, with the addresses in their own family's
width. Highland summed the message alone, so every advertisement the other side
sent was discarded as `bad_checksum`, both nodes timed out, and both became
master. The two implementations had been silently ignoring each other.

The fix is one function, and the evidence is three: a golden vector from a real
implementation in the codec, a suite that runs the two against each other, and the
count of discarded packets — which is now zero in both directions.

Also, and because it is the same failure: **the daemon now logs the first packet
it discards for each reason**, and every thousandth after that. The defect was
invisible for a whole milestone because the count only reached a metric
endpoint, and a metric nobody has enabled is not an explanation.

### IPv6 and multicast, against a second implementation

Four more interoperability scenarios, and the IPv6 half of `G-01`:

- IPv4 multicast, with no peer list on either side: the group is joined on both
  nodes (read from the kernel, not from the configuration that asked for it), one
  node takes the address, and the other stays a backup.
- IPv6 multicast over `ff02::12`, and Keepalived's advertisement is then
  **decoded and re-encoded byte for byte** — the IPv6 counterpart of
  `KEEPALIVED_V4_ADVERTISEMENT`, covering the two things an encoder is most
  likely to get wrong: a sixteen-octet address list, and a pseudo-header with
  sixteen-octet addresses in it. Captured rather than frozen, because the link-local
  source differs on every machine and a checked-in vector could never be compared.
- IPv6 unicast with Highland the master: the peer stays a backup, which is the
  proof that it understood the advertisements.
- IPv6 unicast with Keepalived the master: **not reachable, and the reason is
  recorded rather than worked around.** Keepalived 2.3.3 advertises IPv6 unicast
  with a hop limit of 64; its IPv6 multicast advertisements carry 255, which is why
  the multicast scenario passes. RFC 5798 §5.1.2.3 says a receiver MUST discard such
  a packet, so Highland is right to and a receiver that accepted it would be the
  bug. The test asserts the conforming behaviour — discarded, with the value it
  carried in the reason — rather than skipping.

### A switch for one Keepalived behaviour, and a migration guide

Keepalived 2.3.3 sends IPv6 **unicast** advertisements with a hop limit of 64, and
it rejects `hop_limit` as an unknown keyword in both a `vrrp_instance` block and
`global_defs` — so the peer cannot be corrected from its side. Highland discards
such a packet, as §5.1.2.3 requires, which means a Highland node cannot be a
working backup to a Keepalived master over IPv6 unicast: it never takes over, and
the only clue is a rejection counter.

`[instance.network] allow_unconforming_hop_limit = true` changes that, and
nothing else. It is off by default, it relaxes the hop limit only — version, VRID,
checksum, peer identity and the rate limit are all still enforced — and the daemon
logs a warning at startup when it is on, so an operator can tell from the log
alone that a node is in the degraded mode.

Both sides are tested against Keepalived: with the switch off the packet is
discarded and the value is in the reason; with it on, Highland hears the master,
holds off, and takes the address over inside `Master_Down_Interval`.

`docs/user/migration.md` is new: which family and mode combinations interoperate,
the field mapping, why IPv6 peers are link-local addresses, a cutover procedure
that converts one instance at a time, how to confirm the advertisements are being
heard, and how to roll back. The check that matters in that procedure is
watching for `advertisement received` before trusting a failover, because a node
that is hearing nothing looks exactly like a node that is working.

### Five more defects, all of them IPv6 or multicast

- **A multicast node stepped down against itself.** The kernel loops group traffic
  back to the sending host, so a master heard its own advertisement once a second
  and, at equal priority, lost the tie-break against its own address. A node now
  ignores datagrams from its own source address (`I-50`). Multicast between two
  Highland nodes hid this, because their tie-break went the other way by luck.
- **The received destination was never read.** `IP_PKTINFO` and `IPV6_PKTINFO`
  arrive as *typed* control messages in `nix`, and the reader looked only for
  untyped ones — so the destination was always `None`, the IPv6 checksum was
  verified against a guess, and the narrowing that lets a multicast instance reject
  a datagram addressed to the host rather than to the group was dead code.
- **An IPv6 socket could not be bound to a link-local address at all.** A
  link-local address is only meaningful with an interface, and binding one with a
  scope of zero is `EINVAL` — so IPv6 VRRP, which §5.1.2.1 says is sent *from* the
  link-local address, could not start. The bind now carries the scope.
- **A demoted IPv6 node advertised from the wrong address.** The runner took the
  first IPv6 address on the interface, which on a host with a global address is not
  the link-local the RFC names. The source is now the link-local for IPv6, and the
  IPv4 rule — the primary address — is unchanged, because the difference is the
  RFC's (`I-51`).
- **The daemon would not start during IPv6 duplicate address detection.** A
  link-local the kernel has just created is *tentative* for about a second, and a
  socket cannot bind to a tentative address. Without a retry a node refused to
  start on every boot, with a message about a socket; now it waits, and the error
  that eventually surfaces names the cause.

### A capture that works, because `tcpdump` did not

### Not delivered, and why

- **Keepalived interoperability for IPv6 unicast** needs
  `allow_unconforming_hop_limit = true`; see above and
  `docs/user/migration.md`.
- **Mixed-family instances** are still refused (`V-03`): one instance speaks one
  family per socket and one source address.

### Known limitations

- A node that is killed, frozen, or partitioned while it holds the address keeps
  holding it. Nothing can tell it otherwise, so a survivor takes the address too
  and both nodes have it until the fault heals or the client is fenced. This is
  inherent to VRRP rather than specific to Highland, and it is the reason a load
  balancer belongs in front of the address rather than trusting the address
  alone.
- The netlink, namespace, and chaos tests need `CAP_NET_ADMIN` and namespaces.
  They run in CI jobs on `ubuntu-latest` and in `scripts/linux-tests.sh`, but not
  in the ordinary test job.
- Several errors in `SPEC.md` §18 are aggregated per crate rather than in one
  `HighlandError`, because the CLI and the daemon do not share a dependency set.
  See `docs/architecture.md`.

## [0.0.0]

- Initial repository.
