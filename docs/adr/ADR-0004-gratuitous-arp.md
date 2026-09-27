# ADR-0004: gratuitous announcements, and the two `unsafe` islands they need

- Status: accepted
- Date: 2026-09-27
- Supersedes: nothing. Amends [ADR-0003](ADR-0003-netlink-library.md), which
  deferred this work.

## Context

`SPEC.md` §14.1 makes announcing a claimed address step 5 of becoming `MASTER`:
a gratuitous ARP for each IPv4 VIP, an unsolicited Neighbor Advertisement for each
IPv6 VIP. Without them every neighbour on the segment keeps a cache entry pointing
at the node that held the address last, and traffic to the VIP is a black hole
until that entry ages out — tens of seconds, against a failover measured in
milliseconds.

`NetworkBackend::send_gratuitous_update` existed from Milestone 3 and returned
`NetError::Unsupported` everywhere, because the implementation needs an
`AF_PACKET` socket and `sockaddr_ll` has no safe representation in either
`socket2` or `nix`. ADR-0003 recorded the deferral and suggested a "pnet-style
datalink crate".

## Decision

Two audited `unsafe` islands in one module, `highland-net::gratuitous`, and no new
dependency.

1. **`link_layer_address`** builds the `sockaddr_ll` an `AF_PACKET` send needs.
   `socket2::SockAddrStorage::view_as` does the cast, and `SockAddr::new` takes
   ownership of the storage. Every field the kernel reads is written first, and
   the length passed is the length of the type the storage holds.
2. **`hardware_address`** reads an interface's own hardware address out of the
   `AF_PACKET` entry `getifaddrs` returns. It is the second island because a
   `sockaddr_storage` is a family-agnostic buffer, and reading it as a
   `sockaddr_ll` is a cast rather than a conversion. The read is bounded twice:
   the family is checked to be `AF_PACKET` before the structure is read as one,
   and `sll_halen` is checked against six octets before any byte is copied.

`highland-net` therefore carries its own `[lints]` table with `unsafe_code` set
to `deny` instead of the workspace's `forbid`. The distinction is the point:
`forbid` cannot be lowered to `allow`, so a crate that forbids it cannot grow an
island by accident. `deny` still refuses every `unsafe` block; it lets these two
functions, each documented with its safety argument, say so explicitly. Every
other crate in the workspace still forbids it outright.

The IPv4 announcement is a full Ethernet frame written to an `AF_PACKET` socket,
with the kernel doing no work at that layer: the kernel does not add a header
here, and adding the wrong one is how an announcement ends up invisible to the
neighbour it was for. The frame is an ARP *reply* whose sender and target
protocol addresses are both the address being claimed, broadcast to
`ff:ff:ff:ff:ff:ff`.

The IPv6 announcement is an ICMPv6 Neighbor Advertisement with the override flag
set, sent from a raw ICMPv6 socket bound to the address being claimed and bound
to the device. The kernel builds the IPv6 header and always computes the
ICMPv6 checksum for that protocol, so writing one would be wrong rather than
merely redundant.

## Alternatives rejected

**A datalink crate (`pnet_datalink` or similar).** The crate named in ADR-0003 is
stale: its last release predates the current `pnet` line, and pulling a datalink
stack in for one frame type would add a dependency tree, a buffer-allocation
model, and a promiscuous-capture surface that a routing daemon has no business
owning. The frame is 42 bytes and 30 lines.

**Ask the kernel to do it.** `net.ipv4.conf.<if>.arp_notify` and the behaviour
when an address is added are close but not the same thing: they are per-interface
knobs, they are not per-address, they are silent when they fail, and they cannot
be made to announce a *second* address added to the same interface. Highland
needs to announce exactly the addresses it just claimed, on the interface it
claimed them on, and to know when that did not happen.

**`ioctl(SIOCGIFHWADDR)`.** Equivalent to `getifaddrs` in what it returns and one
`unsafe` block either way, but it needs a `struct ifreq` union built by hand,
which is a second layout to get right. `getifaddrs` is namespace-aware for free,
which matters because the daemon runs in namespaces in its own test suite.

## Consequences

- The announcement is now real, and the state machine's existing treatment of a
  failed one as non-fatal (`R-11`) is exercised rather than theoretical.
- `highland-net` has two documented `unsafe` functions and a lint table that says
  so. A reviewer can grep for `unsafe` in the crate and find exactly four lines.
- The IPv6 path is implemented and its frame is unit-tested, but the daemon's
  IPv6 mode is Milestone 5, so it has not been exercised against a real kernel.
  The IPv4 path is tested end to end.
- `script/linux-tests.sh` gained no new package for this: the test asserts the
  *effect* of an announcement, which is a neighbour's cache entry changing, and
  that needs `iproute2` and nothing else.

## Verified, not assumed

- The frame builders are pure functions with field-by-field tests that run on
  every platform, so a malformed announcement is caught without a kernel.
- The `AF_PACKET` send is tested against a real interface, and the resulting frame
  is confirmed on the wire as `Reply 192.0.2.200 is-at 02:00:00:00:00:01`.
- The takeover test was checked against a build with the announcement disabled:
  it fails, with the neighbour's cache still pointing at the dead master. A test
  that cannot fail is not a test.
- The hardware-address lookup was written first against the wrong field. glibc
  puts the address in the `AF_PACKET` entry's own `sockaddr_ll`, not in a
  netmask field; the passing test is what caught it, which is why the reasoning
  above is written down rather than left in a comment.
