# Upgrading and Rolling Back

Highland is designed so that an upgrade is a rolling operation: replace the
binaries on one node, reload it, watch it, then move to the next. At no point
should both nodes be down, and at no point should a half-applied configuration
be running.

This page is the procedure. The rules it depends on are in the
[operations guide](operations.md).

## Before you start

Three things have to be true:

1. **The current version is healthy.** An upgrade on top of a degraded cluster
   multiplies the variables. Check every node's role and address ownership
   first.
2. **You have the new configuration validated.** Not the old one — the new one,
   on the target version.
3. **You know how to get back.** Keep the previous binaries and the previous
   configuration file. Not in a hurry, not during the incident.

## The rules that make rolling upgrades safe

**Protocol behavior does not change within the 0.x line.** Two nodes running
different 0.x versions interoperate: they speak the same VRRPv3, and the
address moves normally between them. This is what lets you upgrade one node at a
time instead of taking a window.

**Configuration can change, with notice.** A configuration change is always
recorded in [`CHANGELOG.md`](../../CHANGELOG.md) with a migration note. Read it
before you upgrade, not after.

**A reload is all-or-nothing.** If a change cannot be fully applied, the reload
is refused in full. The running configuration does not move halfway.

**Unaffected instances are not interrupted.** An instance the change does not
touch keeps its timers, its address, and its state, uninterrupted. Upgrading
one instance does not disturb its neighbours.

## What can be reloaded, and what cannot

| Change | Applied by a reload? |
|---|---|
| `priority`, `preempt`, `preempt_delay`, `startup_delay` | Yes, in place |
| Health policy | Yes, in place |
| `peers` | Yes, in place |
| `check` — the list of checks | No. The running probes are built from the old list, so the instance is restarted |
| `advertisement_interval` | No. The advertisement timer is armed from the running value |
| `interface`, `vrid` | No. The instance must be restarted |
| Adding or removing a virtual address | No. The instance must be restarted |
| Switching between unicast and multicast | No. The instance must be restarted |
| Adding an instance | Classified, and the reload is applied; the instance itself is started at startup only |
| Removing an instance | No. It relinquishes its addresses and stops |

The rule behind the table: a change is reloadable only if the running instance
can be made to do the new thing without being rebuilt. A timer that was already
armed at the old value, a socket bound to a different interface, and a probe
built from an old list are all things a reload cannot honestly claim to have
changed.

If any instance in the file needs a restart, the reload tells you which ones
before it changes anything. You then choose: drop those changes from this
upgrade, or accept the interruption deliberately, one instance at a time.

## Procedure

### 1. Validate the new configuration

On a machine running the new version, before touching the cluster:

```console
$ highland check-config /etc/highland/config.toml.new
/etc/highland/config.toml.new is valid: 1 instance(s), schema version 1
```

A rejected configuration never reaches a running daemon, so this step is free.

### 2. Read the changelog

Look for the version you are moving to and the one you are moving from. You are
checking for configuration changes, protocol changes, and anything that changes
the meaning of an event you alert on.

### 3. Upgrade one node

```console
$ sudo systemctl stop highland.service
$ sudo install -Dm755 target/release/highland /usr/bin/highland
$ sudo install -Dm755 target/release/highland-daemon /usr/bin/highland-daemon
$ sudo systemctl start highland.service
```

The node rejoins, sees the peer's advertisements, and settles into `BACKUP` if
its priority is lower. **Check that it did**, before touching the next node:

```console
$ highland status
$ highland events --limit 20
```

You are looking for the node to reach a stable `BACKUP` with a reason you
understand. A node that keeps flapping, or that will not settle, is telling you
something about the upgrade. Stop here and investigate.

### 4. Move the address deliberately

If the upgraded node should now serve, do it as a planned action rather than
waiting for a failure:

```console
$ highland relinquish api --yes
```

The peer takes over within one takeover interval — between three and four
advertisement intervals, depending on priority. Clients see a normal address
move, not an outage.

### 5. Repeat on the remaining nodes

One node at a time, verifying between each. Never upgrade the master and its
peer in the same step.

### 6. Confirm the cluster

When the last node is done:

```console
$ highland status          # on every node
$ highland events --limit 50
```

Confirm the roles are what you expect, the address is held by exactly one node,
and no unexpected transition reasons appear in the recent history.

## Rolling back

Rollback is the same procedure in reverse, and it is safe for the same reason:
the protocol does not change.

```console
$ sudo systemctl stop highland.service
$ sudo install -Dm755 /path/to/previous/highland /usr/bin/highland
$ sudo install -Dm755 /path/to/previous/highland-daemon /usr/bin/highland-daemon
$ sudo systemctl start highland.service
```

If the **configuration** is also being rolled back, restore the previous file
and reload it:

```console
$ highland check-config /etc/highland/config.toml.previous
$ sudo install -m 0640 -o root -g root /etc/highland/config.toml.previous /etc/highland/config.toml
$ highland reload
```

The previous configuration is retained by the running daemon, so a rejected
reload is always revertible and the running configuration can always be dumped
for diagnosis. A failed reload never leaves a partial state behind.

## When to consider a full restart instead

A rolling upgrade is the default. Consider taking both nodes down deliberately
instead when:

- The upgrade changes protocol behavior rather than configuration. Within 0.x
  this should not happen, but read the changelog.
- The configuration change forces a restart of every instance anyway, so there
  is no overlap to gain.
- The cluster is small and you would rather have a clean, observable outage at a
  chosen time than a long, staggered one during working hours.

If you do stop everything, remember that with no node holding the address,
clients cannot reach it at all. Time the window and tell people.

## Cross-version notes

- **Within 0.x**: rolling upgrades are supported, as described above.
- **Across major versions**: treat it as a migration. Read the changelog for
  every version you are skipping, not just the one you are moving to.
- **Against Keepalived**: a Highland node and a Keepalived node interoperate at
  the protocol level, which is what makes a migration possible one node at a
  time. Configuration is not interchangeable; see
  [compatibility](compatibility.md).

## If the upgrade goes wrong

Do not improvise with `force-transition`. The states exist to be escaped, but
reaching for one during a failed upgrade hides the cause.

1. Get the state: `highland status` and `highland events` on every node.
2. Roll the node back to the previous version.
3. If the address is held by a node you did not expect, use
   `highland relinquish <instance> --yes` on it and let the protocol settle.
4. Write down the event reasons before you close anything. They are what makes
   the next attempt different from this one.

The [troubleshooting guide](troubleshooting.md) maps symptoms to causes.
