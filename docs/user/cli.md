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

`run` becomes the daemon: the process you start is the one that owns the
address, so it is not something to run in a terminal you might close. It needs
`CAP_NET_ADMIN` and `CAP_NET_RAW`, and without them it refuses to start rather
than sit there claiming to be a VRRP router that cannot move an address.

A reload is a transaction, and it is the same transaction however it is asked
for: `highland reload` over the control socket and `SIGHUP` both call one reload
path, so the two cannot disagree. The file is re-read, every instance is
classified, and the change is applied only if no instance would need a restart. A
refusal names the instance and the change, and nothing is touched. A reloadable
instance is reconfigured in place, so the role, the address, and the running
timers all survive.

Highland handles three signals:

| Signal | What it does |
|---|---|
| `SIGTERM`, `SIGINT` | Stops cleanly, giving up any addresses it holds |
| `SIGHUP` | Re-reads the configuration file and applies it, or refuses the whole reload |
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

### `highland events [--limit <N>] [--follow] [--since <N>]`

The event history. Every role change, health change, address change, reload, and
operator action appears here with the reason for it, recorded as it happened
rather than reconstructed afterwards. The log holds the most recent 4096 events.

| Flag | Default | Meaning |
|---|---|---|
| `--limit <N>` | `50` | How many events to show at a time |
| `-f`, `--follow` | off | Keep asking for new events until interrupted |
| `--since <N>` | `0` | Only events after this sequence number |

Every event carries a sequence number, so `--follow` polls with a cursor rather
than re-reading the buffer: it asks for what it has not seen and holds nothing
open. `--since` is what makes that resumable, and asking for events past the end
returns nothing rather than an error.

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
cannot be issued by accident from a script.

If you need this in production, treat it as an incident: it means the state
machine and the machine disagree, and the reason is worth reading in the event
stream before you force anything.

## Confirmation and exit codes

Commands that change running state name the instance they will affect. Pass
`--yes` to skip the confirmation prompt when scripting. The prompt itself is not
implemented yet, so `--yes` is currently accepted and ignored; the exit code is
not: a refused command exits non-zero, so a script that runs
`highland show nope && deploy` does not go on to deploy.

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
| `reload` | Works, over the socket and by `SIGHUP`, through the same path |
| `events` | Works, and `--follow` resumes from a cursor |

`force-transition` additionally requires the daemon to have been started with
`--enable-force-transition`. The confirmation prompt is not implemented yet:
`--yes` is accepted, and a refusal is a non-zero exit with a message.

Destructive commands (`relinquish`, `force-transition`) act immediately, with or
without a terminal. Treat `--yes` as a way to say "I meant it" in a script, not
as a gate.

A refusal from the daemon is a non-zero exit, not just a printed message: a
script that runs `highland show nope && deploy` must not go on to deploy.

Everything above describes the intended behavior of each command, which is fixed
and will not change. What changes is when it starts working; the
[changelog](../../CHANGELOG.md) records it.
