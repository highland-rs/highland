# Health Checks

A health check answers one question: *should this node keep serving the virtual
address?* Highland probes a service on a schedule and lets the answer change
which node holds the address, so a machine that is up but serving errors gives
the address away before clients notice.

Highland probes natively. It does not run a shell script to decide whether your
application is healthy.

## The model in one minute

A check has a **weight**, and the instance has a **failure policy**.

- A check with `weight = 0` is **observational**. It is recorded, exported, and
  shown in the event stream, and it never changes which node owns the address.
- A check with a non-zero weight is **electoral**. Its weight feeds the
  instance's effective priority, which is what nodes compare when deciding who
  owns the address.

Two numbers keep a single unlucky probe from causing a failover:

| Setting | What it does |
|---|---|
| `failure_threshold` | Consecutive failures before the check is called failing |
| `success_threshold` | Consecutive successes before it is called healthy again |

A probe that times out counts as a failure. A check that is not running at all is
disabled, which is neither passing nor failing.

## Choosing a policy

Three policies exist, and they answer the same question very differently.

### `weighted` — lower your priority

The default. Failing checks subtract from your priority. A node with a lower
effective priority than its peer steps down, so the address moves to a node that
claims to be healthier.

```toml
[instance.health]
failure_policy = "weighted"
minimum_effective_priority = 100
```

```text
effective priority = max(minimum_effective_priority, priority - sum of weights of failing checks)
```

Use it when you want a **preference**, not a rule: the address should go to the
healthier node, but if the unhealthy node is the only one left, it should still
serve.

`minimum_effective_priority` sets the floor. It stops a cascade of failures from
driving a node's priority to nothing while the service is merely degraded. If
the arithmetic reaches zero, the node is not eligible to hold the address at
all, and it stays silent rather than announcing a give-up it did not mean.

### `fail_closed` — give the address up immediately

Any failing blocking check makes the instance ineligible, and an instance holding
the address relinquishes it at once.

```toml
[instance.health]
failure_policy = "fail_closed"
all_checks_required = true
send_zero_priority_advert = true
```

Use it when a node that cannot serve must not keep the address: a database
replica, or anything where traffic to an unhealthy node is worse than a brief
failover.

`all_checks_required` decides what counts as blocking. Left off, only checks
with a non-zero weight block. Turned on, every check blocks, **except** checks
with `weight = 0`, which stay observational by design.

### `manual` — watch, do not act

Health is recorded and exported and never changes the election.

```toml
[instance.health]
failure_policy = "manual"
```

Use it while you are tuning: watch what would have happened, then switch to
`weighted` or `fail_closed` when you trust the result. A check with a non-zero
weight under `manual` is a configuration error, because you have asked for a
signal and then forbidden it from being used.

## Writing a check

```toml
[[instance.check]]
name = "api-ready"
type = "http"
url = "http://127.0.0.1:8080/ready"
expected_status = [200]
timeout = "500ms"
interval = "1s"
failure_threshold = 3
success_threshold = 2
weight = 100
```

```console
$ highland check-config /etc/highland/config.toml
```

A check that this build cannot run is **refused at startup, by name, with the
reason**:

```console
$ highland run --config /etc/highland/config.toml
ERROR highland: instance api: check secure cannot be built: an https check needs
  TLS, which this build does not implement; use a tcp check on the same port
  rather than a check that cannot validate a certificate
```

That is deliberate. A check that cannot be built would otherwise fail on every
interval forever, which is a quieter way to take a node out of service than a
refusal at startup.

Rules that catch most mistakes:

- **Every check type requires its own keys.** An `http` check needs a `url` and a
  non-empty `expected_status`; a `tcp` check needs an `address`. A check missing
  the keys its type requires is rejected.
- **Durations need units**, and `timeout` must not exceed `interval`.
- **Thresholds must be at least 1.**
- **The weights of one instance may not add up to more than 255**, which is the
  whole priority range.

## Choosing check types

| Type | Checks | Use it for | Availability |
|---|---|---|---|
| `tcp` | The port accepts a connection | A dependency that speaks nothing | Works today |
| `http` | Status code is in `expected_status` | A readiness or health endpoint | Works today |
| `unix` | A socket accepts a connection | A local daemon, database socket | Works today |
| `interface` | The link is present, up, and carrying | A dependency on a second link | Works today |
| `https` | The same, with certificate validation | A TLS endpoint | Refused for now |
| `dns` | A record resolves | An upstream dependency | Not implemented |
| `process` | The process exists | Nothing, really: a weak signal | Not implemented |
| `file` | A path exists | A marker file | Not implemented |
| `composite` | Other checks, combined | One signal from several probes | Not implemented |
| `command` | An external program | Almost nothing | Needs `command-checks` |

`https` is **refused rather than downgraded**. A TLS handshake is not something
to reimplement, and a check that connected to port 443 without validating a
certificate would report a service as healthy on the strength of a plaintext
exchange. If you need a TLS dependency watched today, use a `tcp` check on the
port and let something else decide whether the service is actually well.

Two notes on the four that work:

- **`http` sends `GET` and reads the status line**, with the response body
  bounded. It follows no redirects: a `302` is not in `expected_status` unless
  you put it there, because a redirect is usually a misconfiguration rather than
  a healthy service.
- **`unix` and `interface` are the two that need no network** beyond the local
  socket, and `interface` is the one to reach for when the question is "is this
  cable plugged in" rather than "is the service answering".

## Writing checks that behave

A check that flickers causes the address to move back and forth, and clients see
that as an outage. Three habits prevent it:

- **Give a check room to breathe.** `failure_threshold = 3` at `interval = "1s"`
  tolerates two failed probes. `failure_threshold = 1` tolerates none.
- **Make recovery slower than failure** if the service is likely to be
  marginal — `success_threshold = 2` or more stops a flapping service from
  reclaiming the address instantly.
- **Check the thing that is actually broken.** A readiness endpoint that reports
  unhealthy when a *non-critical* dependency is down will move the address for
  no reason, and will keep moving it back. Watch the event stream while you tune
  and you will see exactly which check caused each transition.

## Startup

A node that is starting cannot answer a health probe yet, and a check that fails
during boot would immediately give the address away. Use
`initial_grace_period` to ignore results until the service has had a chance to
start:

```toml
initial_grace_period = "30s"
```

A returning node also completes its `startup_delay` and waits through at least
one full evaluation window before it competes for the address, so it does not
seize the address the instant it comes back.

## Command checks

A check can run an external command instead of probing directly:

```toml
[[instance.check]]
name = "vendor-probe"
type = "command"
command = ["/usr/local/bin/vendor-check", "--quick"]
allow_paths = ["/usr/local/bin/vendor-check"]
weight = 50
```

This is off by default and requires a build with the feature enabled, an explicit
`allow_paths` list of absolute paths, and the command itself to be in that list.
It is the only way a configuration file becomes program execution, which is why
it works that way. Enabling it means accepting that whoever can write your
configuration file can run a program as the daemon; see the
[threat model](threat-model.md).

## Watching health

```console
$ highland show api
$ highland events --follow
```

These reach the daemon over the control socket.

The event stream distinguishes a single failed probe from a check entering the
failing state, from the check recovering, and from the instance becoming
ineligible. That distinction is the fastest way to tell a one-off blip from a
sustained failure.

## A failing check does not move the address by itself

A master whose effective priority drops **keeps advertising**, because that is
what RFC 5798 requires: it does not know its peer is now the better choice. A
backup only takes over from a *live* master when preemption is enabled.

So if you want a failing check to move the address, set `preempt = true` on the
peer:

```toml
# node-a, the node being watched
preempt = false

[instance.check]
name = "api-ready"
type = "http"
url = "http://127.0.0.1:8080/ready"
expected_status = [200]
interval = "2s"
timeout = "1s"
failure_threshold = 3
success_threshold = 2
weight = 100
```

```toml
# node-b, the peer that should take over
preempt = true
```

Without `preempt = true` on the peer, "my check failed and nothing happened" is
the expected outcome, and it surprises people.

## Current state

`tcp`, `http`, `unix`, and `interface` run today, each tested against a real
socket or a real link, and a weighted demotion is driven end to end in the
namespace suite. Not implemented, and refused by name rather than degraded:
`https`, `dns`, `process`, `file`, `composite`.

Two limits worth knowing:

- **Changing the check list needs a restart.** The reload planner says so rather
  than reporting a change as applied and leaving the old probes in place.
- **One instance's checks share one task**, so the probe concurrency limit is
  one per instance rather than a number you configure.

The [configuration reference](configuration.md) lists every key a check accepts.
The [operations guide](operations.md) covers what to do when a health check
causes an unexpected failover.
