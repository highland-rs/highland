# Threat model

## What Highland protects

Highland is a local network service that decides which node owns a virtual IP.
Its security properties are:

1. **Untrusted input never crashes it.** Every parser is total, every allocation
   is bounded, every network operation has a timeout (`S-04`, `S-05`, `S-06`,
   `S-07`).
2. **Local control stays local.** The administrative API is a Unix socket with
   filesystem permissions, never a network listener (`S-02`, `S-03`).
3. **A configuration file is not code.** Command execution is a default-off
   feature, requires an explicit configuration switch, and is restricted to an
   allow-list of absolute paths (`R-06`, `V-21`).
4. **Secrets do not leak.** Redaction applies to logs, metrics, events, and CLI
   output (`S-01`).
5. **Bounded resources.** Every count, size, and rate has a documented limit
   (`L-01` through `L-15`).

## Assets

| Asset | Why it matters |
|---|---|
| VIP ownership | Control of the address clients send traffic to |
| The configuration file | Determines who is trusted as a peer |
| The control socket | Authorizes role changes |
| Check definitions | Can become code execution if misconfigured |
| Event and log output | Can contain addresses and secrets |

## Adversaries

| Adversary | Capability | Mitigation |
|---|---|---|
| Off-segment attacker | Sends VRRP packets | Strict field validation, peer allow-lists, TTL 255 checks, rate limits, no panics |
| On-segment host | Sends valid-looking advertisements | Peer allow-list and VRID checks; note that an on-segment host with the right source address is indistinguishable from a peer, which is why fencing is separate |
| Local unprivileged user | Connects to the control socket | Socket mode `0660`, group ownership, peer credential verification, audit events |
| User who can write the config | Controls peers, checks, and VIPs | File permission check (`V-26`), command checks off by default, no secret expansion in arguments |
| Flooding attacker | Sends packets or requests as fast as the link allows | `L-11` packet rate, `L-12` control request rate, `L-13` reload rate, bounded queues |
| Malformed packet author | Crafted bytes | Fuzzing (Milestone 2), no panics (`I-05`), bounded allocation |

## Trust boundaries

```text
        network segment                 host                     process
  ┌────────────────────────┐   ┌──────────────────────┐   ┌──────────────────┐
  │ VRRP packets          │   │ configuration file   │   │ control socket   │
  │ (untrusted)            │   │ (trusted, 0640)      │   │ (local, 0660)    │
  └───────────┬────────────┘   └──────────┬───────────┘   └────────┬─────────┘
              │ validated                  │ parsed and validated     │ authenticated
              ▼                            ▼                          ▼
        highland-vrrp                highland-config            highland-control
              └──────────────┬──────────────┴──────────────┬───────────┘
                             ▼                             ▼
                      highland-core  ◀── actions ──  highland-net (CAP_NET_ADMIN)
```

The parser is the first thing untrusted bytes meet, and it is the only place
where untrusted data becomes domain data. Everything downstream assumes
validated input.

## Out of scope

- An attacker who already controls the host or the configuration file.
- Denial of service from a position that can already saturate the host's network
  stack.
- Confidentiality of traffic to the VIP. Highland moves addresses; it does not
  encrypt application traffic.
- Protection against a determined on-segment host spoofing a peer's address.
- Split-brain. It is a protocol limitation, not a vulnerability, and Highland
  does not claim otherwise.
- `command` checks, when an operator deliberately enables them. Enabling them
  means accepting that the configuration can execute a program.

## Privilege

Required: `CAP_NET_ADMIN`, `CAP_NET_RAW`. Not required: unrestricted root, and
no helper process. Privilege separation is post-1.0; today the daemon holds
those two capabilities for its whole lifetime.

## Known weaknesses

| Weakness | Status |
|---|---|
| An on-segment host that can send from a peer's address can inject advertisements | Inherent to VRRP; peer allow-lists raise the bar; fencing is post-1.0 |
| A VIP conflict with a host outside Highland's knowledge | Detected only through gratuitous ARP/NA observation; reported, not prevented |
| `command` checks re-enable code execution through configuration | Documented as operationally unsafe; off by default; allow-listed paths only |

## Reporting

See [`SECURITY.md`](../SECURITY.md).
