# Configuration reference

The normative rules are `SPEC.md` §10. This document is the operator-facing
reference. Every key, every default, and every validation rule appears here.

## Format

TOML, one file, no includes. TOML is the only accepted format; there is no
YAML and no JSON input (`SPEC.md` §9.5).

Two things are refused at load time:

- a world-writable file, unless `--allow-insecure-config` is passed (`V-26`)
- a file above 4 MiB (`L-06`)

Unknown keys are rejected. A typo is a startup failure, not a silent no-op.

## Durations

Durations are a number and a unit: `ns`, `us`, `ms`, `s`, `m`, `h`. Components
add up, so `1m30s` is ninety seconds. A bare number is rejected.

## Top level

| Key | Type | Default | Notes |
|---|---|---|---|
| `schema_version` | integer | required | `1` is the only accepted value (`V-28`) |
| `node.name` | string | required | Must not be blank (`V-32`) |
| `logging.level` | `error`\|`warn`\|`info`\|`debug`\|`trace` | `info` | |
| `logging.format` | `text`\|`json` | `text` | |
| `logging.redact` | array of key paths | `[]` | Extra patterns for the redactor (`S-01`) |
| `metrics.enabled` | bool | `false` | |
| `metrics.listen` | socket address | — | Required when enabled, forbidden when not (`V-30`) |
| `control.socket` | path | `/run/highland/control.sock` | Created with mode `0660` (`S-03`) |
| `control.group` | group name | — | Ownership of the socket |
| `control.verify_peer_credentials` | bool | `true` | |
| `instance` | array | — | At least one is required (`V-31`), at most 256 (`V-27`) |

## `[[instance]]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `name` | string | required | Unique across the file (`V-05`) |
| `interface` | string | required | Must exist unless `defer_interface_binding` (`V-22`) |
| `defer_interface_binding` | bool | `false` | |
| `vrid` | integer | required | `1..=255`; `0` is invalid (`V-01`) |
| `priority` | integer | `100` | `1..=255`; `0` is reserved for relinquishment (`V-02`) |
| `advertisement_interval` | duration | `1s` | `10ms..=40.95s`, the range of the 12-bit centisecond `Max Adver Int` field (`V-04`) |
| `preempt` | bool | `true` | |
| `preempt_delay` | duration | `0s` | Forbidden when `preempt = false` (`V-10`) |
| `startup_delay` | duration | `0s` | Delay before entering election |
| `vip` | array | — | At least one is required (`V-11`) |
| `network` | table | unicast defaults | See below |
| `health` | table | weighted defaults | See below |
| `check` | array | `[]` | At most 64 (`V-25`), names unique (`V-29`) |

An instance is bound to exactly one interface and, before 1.0, to exactly one
address family (`V-03`).

### `[[instance.vip]]`

| Key | Type | Notes |
|---|---|---|
| `address` | CIDR string | Prefix 0 is rejected (`V-13`); the same address may not appear twice or in two instances (`V-12`). One instance holds the addresses of one family; mixing them is rejected (`V-03`), because an instance speaks one family per socket and one source address |

### `[instance.network]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `mode` | `unicast`\|`multicast` | `unicast` | In `multicast` mode there is no peer list: the group is where the peers are, and the group membership is what authorises a node's advertisements |
| `peers` | array of IP | `[]` | Family is inferred per entry, and a list may hold both families: peers of the other family are ignored. A VIP family with no peer is rejected in unicast mode (`V-07`). A local address is rejected (`V-08`); a multicast address is rejected (`V-09`); at most 255 (`L-03`) |
| `multicast.group` | IP | per family | Unset, so the group is `224.0.0.18` for an IPv4 instance and `ff02::12` for an IPv6 one, which is not the same address. A configured group must be a multicast address of the instance's family, or the instance is refused (`V-24`) |
| `multicast.ttl` | integer | `255` | Anything else is rejected (`V-24`). The multicast TTL is a different socket option from the unicast one and defaults to 1, which every receiver would discard |

### `[instance.health]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `failure_policy` | `fail_closed`\|`weighted`\|`manual` | `weighted` | See below |
| `minimum_effective_priority` | integer | `1` | Only meaningful under `weighted` (`V-16`, `V-17`) |
| `all_checks_required` | bool | `false` | Only meaningful under `fail_closed` (`V-18`) |
| `send_zero_priority_advert` | bool | `true` | Only meaningful under `fail_closed` (`V-18`) |
| `debounce` | duration | `0s` | Extra debounce on top of the check thresholds |

The three policies, exactly:

- `weighted`: `effective = max(minimum_effective_priority, priority - Σ weight of
  failing checks)`. A node whose effective priority reaches 0 is not eligible
  (`I-23`).
- `fail_closed`: any failing blocking check makes the instance ineligible, and a
  `MASTER` relinquishes immediately. "Blocking" means every check when
  `all_checks_required` is set, otherwise only checks with a non-zero weight.
- `manual`: health is recorded and exported but never changes election. A check
  with a non-zero weight under `manual` is a configuration error (`V-19`).

A check with `weight = 0` is observational: it is exported and emitted, and it
never affects election, even under `all_checks_required` (`R-14`).

### `[[instance.check]]`

| Key | Type | Default | Notes |
|---|---|---|---|
| `name` | string | required | Unique within the instance (`V-29`) |
| `type` | `tcp`\|`http`\|`https`\|`dns`\|`unix`\|`process`\|`interface`\|`file`\|`composite`\|`command` | required | Unknown types are rejected (`V-23`) |
| `weight` | integer | `0` | `0` is observational; the total may not exceed 255 (`V-20`) |
| `interval` | duration | required | Must be positive, and at least `timeout` (`V-14`) |
| `timeout` | duration | required | Must be positive (`V-14`) |
| `failure_threshold` | integer | required | At least 1 (`V-15`) |
| `success_threshold` | integer | required | At least 1 (`V-15`) |
| `initial_grace_period` | duration | `0s` | |
| `retry_interval` | duration | `0s` | `0s` means one attempt per interval |

Type-specific keys, all required for the type they belong to (`V-23`):

| Type | Required keys |
|---|---|
| `tcp` | `address` |
| `http`, `https` | `url`, `expected_status` |
| `dns` | `record` |
| `unix`, `file` | `path` |
| `process` | `process` |
| `interface` | `interface` |
| `composite` | — |
| `command` | `command`, plus `allow_paths` |

### Which types run today

`tcp`, `http`, `unix`, and `interface` are implemented and tested. The rest are
**refused by name at startup**, with the reason, rather than accepted and quietly
degraded:

| Type | Why not |
|---|---|
| `https` | Needs a TLS stack and certificate validation. A check that connected to port 443 without validating a certificate would report a service as healthy on the strength of a plaintext exchange, so an `https` check is refused rather than downgraded to a `tcp` one. |
| `dns` | Not implemented. |
| `process` | Not implemented. Existence is a weak signal that says nothing about readiness. |
| `file` | Not implemented. |
| `composite` | Not implemented. |
| `command` | Needs the `command-checks` feature and an `allow_paths` list. |

A check that cannot be built is a **configuration error at startup**, and that is
the point: a check that fails on every interval forever is a quieter way to take a
node out of service than a refusal.

### A failing check does not give the address up by itself

A master whose effective priority drops keeps advertising, which is what RFC 5798
requires: a backup only takes over from a *live* master when preemption is
enabled. So if you want a failing check to move the address, set `preempt = true`
on the peer:

```toml
# The node being watched. Its weight is what a failure costs.
preempt = false

[instance.check]
name = "api"
type = "tcp"
weight = 100
interval = "2s"
timeout = "1s"
failure_threshold = 3
success_threshold = 2
address = "10.0.0.10:8080"

# And on the peer that should take over when this one is demoted:
# preempt = true
```

`weight = 0` makes a check observational: it is reported and exported, and it
never affects election.

`command` checks additionally require a binary built with the `command-checks`
feature, an `allow_paths` list of absolute paths, and the command itself to be in
that list (`V-21`). This is the only way a configuration file becomes code
execution, which is why it is off by default.

## Validation rules and their tests

Every rule has a test in `crates/highland-config/tests/validation.rs`, named
`vNN_...`. The list is:

| Rule | Rejects |
|---|---|
| `V-01` | VRID 0 |
| `V-02` | Configured priority 0 |
| `V-03` | An instance mixing address families before 1.0 |
| `V-04` | Advertisement interval outside `10ms..=40.95s` |
| `V-05` | Duplicate instance names |
| `V-06` | Two instances sharing an `(interface, vrid)` pair |
| `V-07` | A VIP family with no peer of that family in unicast mode |
| `V-08` | A peer configured on this node |
| `V-09` | A multicast address used as a unicast peer |
| `V-10` | `preempt_delay` set while `preempt = false` |
| `V-11` | An instance with no VIP |
| `V-12` | A VIP shared between instances, or repeated within one |
| `V-13` | A malformed VIP, a prefix above the family maximum, or prefix 0 |
| `V-14` | A zero timeout, a zero interval, or a timeout above the interval |
| `V-15` | A zero threshold |
| `V-16` | `minimum_effective_priority` under `fail_closed` |
| `V-17` | `minimum_effective_priority` or `all_checks_required` under `manual` |
| `V-18` | `all_checks_required` or `send_zero_priority_advert` under `weighted` |
| `V-19` | An electoral check under `manual` |
| `V-20` | A total check weight above 255 |
| `V-21` | A command check without the feature, an allow-list, or an absolute path |
| `V-22` | A missing interface, unless binding is deferred |
| `V-23` | A check missing the keys its type requires, or an unknown type |
| `V-24` | A multicast TTL other than 255 |
| `V-25` | More than 255 VIPs, 255 peers, or 64 checks in one instance |
| `V-26` | A world-writable configuration file |
| `V-27` | More than 256 instances |
| `V-28` | An unsupported `schema_version` |
| `V-29` | Two checks with the same name in one instance |
| `V-30` | `metrics.enabled` and `metrics.listen` disagreeing |
| `V-31` | No instances at all |
| `V-32` | A blank `node.name` |

## Reload

`highland reload` and `SIGHUP` follow the same nine steps in `SPEC.md` §10.5. A
rejected reload leaves the running configuration untouched (`I-09`). Instances
classified `Unchanged` are not interrupted (`I-10`).

## Checking a file

```console
$ highland check-config /etc/highland/config.toml
/etc/highland/config.toml is valid: 1 instance(s), schema version 1
```

`check-config` reports every violation at once, each prefixed with the
specification rule and the configuration key that carries it.
