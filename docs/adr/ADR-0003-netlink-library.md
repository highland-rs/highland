# ADR-0003: Netlink access

- Status: proposed
- Date: 2026-09-27
- Blocks: `SPEC.md` `B-02`, `R-04`, Milestone 3

## Context

`highland-net` must add and remove addresses, read interface state, watch
link-state notifications, send gratuitous ARP and unsolicited Neighbor
Advertisements, and manage multicast membership. All of that sits behind the
`NetworkBackend` trait, so the choice of library is contained.

The candidates are a direct `rtnetlink` implementation, a higher-level crate,
and shelling out to `ip`, which is excluded: the project does not shell out for
core functionality.

## Decision

Not yet made. The trait boundary is drawn, and the crate currently contains no
Linux-specific code at all, so the decision can be deferred without rework.

## Constraints on the eventual decision

- No `unsafe` in the workspace. A library requiring `unsafe` needs an ADR of its
  own and a specification change (`unsafe_code = "forbid"` today).
- Address add and remove MUST confirm by read-back (`I-19`), not by request
  acknowledgment alone.
- The implementation MUST be testable in a network namespace, so that
  `crates/highland-daemon/tests/network_ns/` can exercise it.
- Netlink error codes MUST map onto `NetError` variants that name the interface
  and the address (`R-23`).
