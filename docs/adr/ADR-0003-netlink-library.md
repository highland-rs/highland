# ADR-0003: Netlink access

- Status: accepted
- Date: 2026-09-27
- Resolves: `SPEC.md` Appendix B, `B-02`; unblocks `R-04` and Milestone 3

## Context

`highland-net` must add and remove addresses, read interface state, watch
link-state notifications, and send gratuitous updates. All of that sits behind
the `NetworkBackend` trait, so the choice of library is contained.

Three constraints shaped the search, and they are not the usual ones:

1. **No `unsafe` in this workspace.** `unsafe_code = "forbid"` is a workspace
   lint. A dependency may of course contain `unsafe`; our code may not. This rules
   out writing netlink message construction by hand, because `sockaddr_nl` and the
   netlink message format can only be built through raw pointers.
2. **The workspace must still build off Linux.** Highland targets Linux only, but
   `cargo test --workspace` and the CI matrix include macOS, and contributors work
   on it. A dependency that fails to compile off Linux would break the
   contributor experience for a Linux-only product.
3. **The API must be able to confirm an effect.** Address add and remove MUST
   confirm by read-back, not by request acknowledgment (`I-19`).

## Options considered

| Option | Verdict |
|---|---|
| Shell out to `ip` | Excluded. The project does not shell out for core functionality, and a subprocess per address change is a reliability and security problem. |
| `rtnetlink` | **Chosen.** |
| `netlink-packet-route` plus `netlink-socket` | Rejected for now. It is the lower-level, more flexible pairing, and `netlink-socket` was not resolvable in the registry at the time of writing. It is the escape hatch if `rtnetlink` stops covering a need. |
| Hand-rolled netlink over `libc` | Rejected. It requires `unsafe` here, and it would be code to maintain and fuzz for no gain at this stage. |
| `tokio`-first async netlink | Rejected as a separate dependency graph. `rtnetlink` has a `tokio` feature, which is the same thing with less to keep current. |

## Decision

Use **`rtnetlink` 0.23 with its `tokio` feature**, declared as a Linux-only
dependency:

```toml
[target.'cfg(target_os = "linux")'.dependencies]
rtnetlink = { version = "0.23", features = ["tokio"] }
socket2 = { version = "0.6", features = ["all"] }
```

The Linux implementation lives in `#[cfg(target_os = "linux")]` modules. Every
other target gets a backend whose operations return
`NetError::Unsupported { .. }`, so the trait, the executor, and the tests compile
and run everywhere.

### Why `rtnetlink`

- It covers exactly the four operations Milestone 3 needs with typed requests and
  responses: list links, get a link, add an address, delete an address, plus
  address and link subscriptions.
- It is actively maintained. Version 0.23 was released within a month of this
  decision and its declared MSRV is 1.75, below this workspace's 1.85.
- Its `tokio` feature is exactly the integration we want: the asynchronous handle
  without pulling in `async-global-executor` or `smol_socket`.
- The `unsafe` stays in its dependency tree, so the workspace lint still holds.

### Why the target gate is not optional

`rtnetlink` 0.23 depends on `netlink-sys` 0.9, which references `libc::PF_NETLINK`,
`libc::SOCK_CLOEXEC`, and `libc::sockaddr_nl` without cfg-gating them. Those
symbols exist only on Linux, so the dependency **fails to compile on macOS** even
though the whole product is Linux-only. Declaring it under
`[target.'cfg(target_os = "linux")'.dependencies]` is what keeps
`cargo test --workspace` working for a contributor on a Mac.

This is recorded here because it is not obvious from the manifest, and because
"just add the dependency" is the natural mistake.

### `socket2` for the raw sockets

VRRP needs a `SOCK_RAW` socket bound to IP protocol 112, which has no
`socket2` constant and is created as `Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::from(112)))`.
The `all` feature is required for `Type::RAW`. This is verified to compile
without `unsafe` in our code.

Gratuitous ARP needs an `AF_PACKET` socket, whose `sockaddr_ll` has no safe
`socket2` representation. That is deferred to Milestone 4 with a
`pnet`-style datalink crate, and until it lands the operation returns
`NetError::Unsupported`. The state machine already treats a gratuitous-update
failure as non-fatal, so nothing else changes.

## Consequences

- `highland-net` gains a Linux-only dependency edge, and a second dependency for
  raw sockets. Both are recorded in `docs/architecture.md`.
- The Linux backend is the only place in the workspace that talks to the kernel.
  Everything else in Highland is portable and testable, which is the property the
  dependency graph in `SPEC.md` §9 exists to protect.
- If `rtnetlink` ever stops covering a need, the fallback is
  `netlink-packet-route` plus `netlink-socket`, adopted as a new ADR. The trait
  boundary means that swap is confined to one module.
