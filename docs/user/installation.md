# Installation

Highland is a single binary pair with no runtime dependencies beyond Linux
itself. There is nothing to compile on the target machine, no interpreter, and
no mandatory connection to a management service.

## What you need

| Requirement | Why |
|---|---|
| Linux on `x86_64` or `aarch64` | It uses Linux networking directly |
| `CAP_NET_ADMIN` | To add and remove the virtual address |
| `CAP_NET_RAW` | For the raw VRRP socket, and for the `AF_PACKET` socket a gratuitous ARP needs |
| A configuration file you can read | It is the only input |
| A writable `/run/highland` if you use the control socket | The socket lives there |

Highland does not need unrestricted root, does not talk to a database, and does
not modify your firewall.

## Build from source

Requires a Rust toolchain with edition 2024 support; the minimum supported
version is 1.85.

```console
$ git clone https://github.com/highland-rs/highland
$ cd highland
$ cargo build --release --workspace
```

Two binaries land in `target/release/`:

| Binary | Role |
|---|---|
| `highland` | The command you run, including `highland run` |
| `highland-daemon` | The process `highland run` starts |

Keep them together. `highland run` looks for `highland-daemon` next to itself and
fails with a clear message if it is missing.

## Install

```console
$ sudo install -Dm755 target/release/highland /usr/bin/highland
$ sudo install -Dm755 target/release/highland-daemon /usr/bin/highland-daemon
$ sudo install -d -m 0750 /etc/highland
$ sudo install -Dm644 /path/to/config.toml /etc/highland/config.toml -o root -g root -m 0640
$ sudo install -d -m 0755 /run/highland
```

Keep the configuration file at `0640` or tighter, owned by root. Highland refuses
a world-writable configuration file, because a file anyone can write is a file
anyone can use to nominate a trusted peer.

To confirm the installation before going further:

```console
$ highland version
highland 0.1.0
$ highland check-config /etc/highland/config.toml
/etc/highland/config.toml is valid: 1 instance(s), schema version 1
```

## Install and run the service

A unit file is supplied:

```console
$ sudo install -Dm644 deploy/systemd/highland.service /etc/systemd/system/highland.service
$ sudo systemctl daemon-reload
$ sudo systemctl enable --now highland.service
$ systemctl status highland.service
$ journalctl -u highland.service -f
```

The supplied unit is a starting point. Check these before you rely on it:

- **`Type=simple`, not `Type=notify`.** The daemon does not send an `sd_notify`
  readiness message, and a unit that waits for one it will never receive fails
  to start after `TimeoutStartSec`. `Type=simple` is correct here because the
  daemon loads and validates its configuration before it binds anything: a
  process that has started is a process whose configuration was accepted.
- **Capabilities.** `AmbientCapabilities` must cover what your deployment
  actually opens. The supplied pair is the documented minimum.
- **Coexisting with Keepalived.** Do not add `Before=keepalived.service` unless
  you are deliberately running both during a migration. Two VRRP implementations
  on one node will fight over the address.
- **The control socket** defaults to `/run/highland/control.sock`, which the
  unit creates via `RuntimeDirectory=highland`.

Reloading through systemd is `systemctl reload highland.service`, which sends
`SIGHUP`. That applies the change, or refuses the whole reload and says why. It
is the same operation as `highland reload`, so an operator who cannot send
signals is not at a disadvantage.

## Run it without systemd

Highland is an ordinary foreground process. It does not require systemd, and it
runs under OpenRC, in a container, or from a shell. An OpenRC script is supplied
in `deploy/openrc/highland`.

```console
$ highland run --config /etc/highland/config.toml
```

## Containers

An image definition is supplied in `deploy/containers/Dockerfile`. It is intended
for testing Highland against another instance, not for production: VRRP needs a
real layer-2 segment, and a bridge inside a single container does not provide
one. Running two instances in two containers on one host will not fail over the
way you expect.

## Permissions and hardening

The supplied unit sets a conservative baseline. If you harden it further, keep
these working:

- `CAP_NET_ADMIN` and `CAP_NET_RAW` must survive; without them the daemon cannot
  manage addresses.
- The configuration file must stay readable.
- The state directory must stay writable.
- Nothing else needs write access. Highland does not need a writable filesystem
  after startup, apart from its own state and run directories.

## Upgrading

Upgrades are covered in [Upgrading and rolling back](upgrading.md).

## Uninstalling

```console
$ sudo systemctl disable --now highland.service
$ sudo rm -f /etc/systemd/system/highland.service
$ sudo systemctl daemon-reload
$ sudo rm -f /usr/bin/highland /usr/bin/highland-daemon
$ sudo rm -f /etc/highland/config.toml
```

Check by hand that no virtual address is left behind:

```console
$ ip -brief address show
```

If an address is still configured after the daemon has stopped, remove it
explicitly. Nothing else cleans it up for you.

## Next steps

- [Getting started](getting-started.md) — a first working configuration
- [Configuration reference](configuration.md) — every key and rule
- [Operations guide](operations.md) — signals, events, and the failure playbook
