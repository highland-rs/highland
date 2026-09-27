# Scripts

## `linux-tests.sh`

Runs the checks that only a real Linux kernel can perform, in a container:
formatting, clippy, the whole test suite, rustdoc, and the Netlink tests against
a real kernel with `CAP_NET_ADMIN`.

```console
$ scripts/linux-tests.sh            # everything
$ scripts/linux-tests.sh --no-net   # everything except the privileged tests
```

It exists because `highland-net`'s Netlink backend is selected by
`cfg(target_os = "linux")`. On a non-Linux host that module is never compiled,
so a mistake in it can sit there unnoticed until someone tries to run the
daemon. The first time this was run, the module did not compile at all, and
after it compiled it had a bug that only a live socket could reveal.

The container writes nothing back to the tree: the source is mounted read-only
and the cargo and target directories are named volumes.
