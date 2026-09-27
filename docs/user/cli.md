# Command Reference

Every `highland` command, what it does, and whether it works in the current
build.

If you only want to know what to type, start with the [common
commands](#common-commands) below.

## Common commands

```console
$ highland check-config /etc/highland/config.toml   # validate before restarting
$ highland run --config /etc/highland/config.toml   # run in the foreground
$ highland status                                   # what does each instance own?
$ highland events --follow                          # watch role changes live
$ highland reload                                   # apply a configuration change
$ highland relinquish api --yes                     # hand the address over on purpose
```

## Global flags

These work with any command.

| Flag | Default | Meaning |
|---|---|---|
| `--socket <PATH>` | `/run/highland/control.sock` | Which daemon to talk to. A Unix socket path may be at most 107 bytes |
| `--json` | off | Machine-readable output, where the command supports it |
| `--version` | — | Print the version |
| `--help` | — | Print usage |

`--json` changes only how a result is printed, never what is asked for. Use it
for anything you intend to parse.

## Commands that work today

### `highland version`

Prints the version. Needs nothing else.

### `highland check-config <PATH>`

Parses and validates a configuration file and exits, without starting anything.

```console
$ highland check-config /etc/highland/config.toml
/etc/highland/config.toml is valid: 1 instance(s), schema version 1
```

It reports every problem at once, labelled with the rule that rejected it:

```text
highland: validating /etc/highland/config.toml: [V-01] instance.api.vrid: 0 is not a valid VRID; use 1..=255
```

The rule label is a stable reference, so if you search for it you will find the
entry in the [configuration reference](configuration.md). Run this command
before every reload and before every restart. A rejected configuration never
reaches the running daemon.

Two rules are checked by the daemon rather than by this command, because they
depend on the machine: whether a peer you listed is this node's own address, and
whether the interface you named exists. `check-config` deliberately does not
fail on those.

### `highland run [--config <PATH>] [--allow-insecure-config]`

Runs Highland in the foreground, loading the configuration you give it.

| Flag | Default | Meaning |
|---|---|---|
| `--config <PATH>` | `/etc/highland/config.toml` | The configuration file to load |
| `--allow-insecure-config` | off | Accept a world-writable configuration file |

Only pass `--allow-insecure-config` for a file on a trusted filesystem, such as
one generated at boot. Highland refuses a world-writable file by default because
anyone who can write it can name themselves as a trusted peer.

The configuration is loaded and validated first, so a broken file reports the
broken file rather than a problem with the daemon. Each instance then binds a raw
VRRP socket, opens the control socket, and runs until a signal arrives.

`highland run` **replaces** itself with the daemon, so a signal sent to
`highland run` reaches the process that owns the sockets.

```console
$ highland run --config /etc/highland/config.toml
INFO highland started node=node-a instances=1
INFO instance started instance=api vrid=42 source=192.0.2.11 peers=1
INFO role changed instance=api role=BACKUP reason=startup effective_priority=150
INFO control socket listening socket=/run/highland/control.sock
```

On a build with no raw socket the daemon refuses to start, and says so. A process
that claimed to be a VRRP router while sending nothing would be worse than one
that declines to run.

A reload is a transaction. `highland reload` is not wired to the socket in this
build, but `SIGHUP` is: the file is re-read, every instance is classified, and
the change is applied only if no instance would need a restart. A refusal names the
instance and the change, and nothing is touched. A reloadable instance is
reconfigured in place, so the role, the address, and the running timers all
survive.

Highland handles three signals:

| Signal | What it does |
|---|---|
| `SIGTERM`, `SIGINT` | Stops cleanly, giving up any addresses it holds |
| `SIGHUP` | Re-reads and re-validates the configuration file. Does not apply it yet |
| A second `SIGTERM` | Ignored; the first shutdown is already under way |

The `run` command starts the `highland-daemon` process, so both binaries must be
installed together.

## Commands that need a running daemon

Everything below talks to the daemon over a local socket. They fail with a clear
message if the daemon is not running, and never fall back to reading the
configuration file or inspecting the machine.

**These commands do not work in the current build**, because the socket is not
open yet. See [Current state](#current-state).

### `highland status`

The first command to reach for. One line per instance: its name, its role, the
priority you configured, the priority it is actually using after health
adjustments, and whether it is currently holding its addresses.

```text
node node-a (generation 7)
  api              MASTER   priority 150 (effective 100) [owning vips]
```

A node that is `BACKUP` with a lower effective priority than its peer is
behaving correctly.

### `highland show <INSTANCE>`

Everything about one instance: role, both priorities, the address set, whether
the addresses are actually held, the state of each health check, peer
reachability, and the time remaining on the takeover and preemption timers. A
timer that is not running shows as `null` rather than as a misleading zero.

### `highland events [--limit <N>] [--follow]`

The event stream. Every role change, health change, address change, reload, and
operator action appears here with the reason for it.

| Flag | Default | Meaning |
|---|---|---|
| `--limit <N>` | `50` | How many recent events to show first |
| `-f`, `--follow` | off | Keep streaming |

This is the most useful command during an incident: the reason field tells you
why an instance changed role instead of leaving you to infer it. The
[operations guide](operations.md) lists every reason you will see and what to
check for each.

### `highland reload`

Re-reads the configuration file and applies it. Identical to sending `SIGHUP` to
the daemon.

The reload is all-or-nothing. If any instance cannot be changed in place, the
whole reload is refused and nothing changes, and you get the reason. Instances
that are unaffected are not interrupted: their timers, their addresses, and
their state continue uninterrupted.

### `highland pause <INSTANCE>` / `highland resume <INSTANCE>`

`pause` takes an instance out of the election without stopping the daemon. It
gives up any addresses it holds, stops advertising, and waits. `resume` puts it
back, after a fresh startup delay so it does not seize the address the instant
it returns.

Use `pause` for planned maintenance on a single instance, and `relinquish` when
you want the address to move to the peer.

### `highland relinquish <INSTANCE>`

Asks a master instance to hand its addresses over deliberately: announce the
give-up, remove the addresses, and become a backup. The peer takes over within
one takeover interval.

This is the graceful version of a failover. Prefer it to stopping the process
when you are moving work between nodes.

### `highland force-transition <INSTANCE> --role <ROLE> --enable`

Forces a role change, bypassing the normal rules. It exists for one situation:
an instance stuck in a state it will not leave on its own.

| Flag | Meaning |
|---|---|
| `--role <ROLE>` | `init`, `backup`, `master`, `fault`, or `disabled` |
| `--enable` | Required. Acknowledges that this changes running state |

The daemon must also have been started with `--enable-force-transition`, so this
cannot be issued by accident from a script. That daemon flag does not exist yet.

If you need this in production, treat it as an incident: it means the state
machine and the machine disagree, and the reason is worth reading in the event
stream before you force anything.

## Confirmation and exit codes

Commands that change running state print the instance they will affect and ask
for confirmation. Pass `--yes` to skip the prompt when scripting. The prompt is
not implemented yet, so `--yes` is currently accepted and ignored.

| Outcome | Exit code |
|---|---|
| Success | `0` |
| Any error, including a refusal from the daemon | `1` |
| An unreachable daemon | `1` |

Errors go to standard error as `highland: <message>`, so a script can capture
them.

## Current state

| Command | State |
|---|---|
| `version` | Works |
| `check-config` | Works |
| `run` | Works, and becomes the daemon rather than supervising it |
| `status`, `show` | Work |
| `pause`, `resume`, `relinquish` | Work |
| `force-transition` | Works, and needs the daemon started with `--enable-force-transition` |
| `reload` | `SIGHUP` applies the change; over the socket it answers "the reload runs on the signal loop" |
| `events` | Answers "not implemented" rather than hanging |

`force-transition` additionally requires the daemon to have been started with
`--enable-force-transition`, and refuses without an explicit confirmation.

A refusal from the daemon is a non-zero exit, not just a printed message: a
script that runs `highland show nope && deploy` must not go on to deploy.

Everything above describes the intended behavior of each command, which is fixed
and will not change. What changes is when it starts working; the
[changelog](../../CHANGELOG.md) records it.
