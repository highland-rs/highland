# Highland Documentation

Everything you need to install, configure, run, and operate Highland.

## Start here

| If you want to | Read |
|---|---|
| Install it | [Installation](installation.md) |
| Get it running for the first time | [Getting started](getting-started.md) |
| Look up a configuration key or an error message | [Configuration reference](configuration.md) |
| Decide how health should affect failover | [Health checks](health-checks.md) |
| Run a command and understand what it did | [Command reference](cli.md) |
| Fix something that is wrong | [Troubleshooting](troubleshooting.md) |
| Look up an event or a signal | [Operations guide](operations.md) |
| Upgrade or roll back | [Upgrading and rolling back](upgrading.md) |
| Move off Keepalived | [Compatibility](compatibility.md) |
| Report a vulnerability | `SECURITY.md` in the repository root |
| Know exactly what the software promises | [Specification](../SPEC.md) |

## What Highland is

Highland keeps a virtual IP address available by moving it between machines. It
speaks VRRPv3, the protocol Keepalived uses, so it works alongside existing
implementations on the same segment.

It is built around three commitments:

- **It never claims an address it does not hold.** A node confirms ownership in
  the kernel before it advertises, and says so plainly when it cannot take
  ownership.
- **It explains itself.** Every role change carries a reason, visible in the
  event stream, the logs, and the metrics. You are never left inferring why an
  address moved.
- **It is honest about its limits.** Highland is a layer-2 tool. It survives a
  machine failing. It does not survive a network partition that separates the
  nodes from each other while leaving both able to reach clients, and it does not
  pretend otherwise. Read [split-brain
  behavior](compatibility.md#split-brain-behavior) before you deploy.

## The documents

| Document | What it covers |
|---|---|
| [Installation](installation.md) | Requirements, building, installing, systemd and OpenRC, containers, permissions, uninstalling |
| [Getting started](getting-started.md) | A first configuration, a two-node setup, running it, changing it later |
| [Configuration reference](configuration.md) | Every key, every default, and every rule that rejects a file |
| [Health checks](health-checks.md) | Check types, weights, choosing a policy, writing checks that do not flap |
| [Command reference](cli.md) | Every command, its flags, and what it does |
| [Troubleshooting](troubleshooting.md) | Symptoms first, then the checks that find each cause |
| [Operations guide](operations.md) | Signals, every event and reason, the failure playbook, split brain, upgrade and rollback |
| [Upgrading and rolling back](upgrading.md) | The rolling upgrade procedure, what a reload can and cannot change, and rollback |
| [Compatibility](compatibility.md) | Keepalived mapping, protocol differences, IPv6, diagnosing with packet captures |
| [Threat model](threat-model.md) | What Highland trusts, what it refuses, and what is out of scope |
| [Specification](../SPEC.md) | The complete requirements, with stable references for tests and issues |

Three of these overlap on purpose. [Troubleshooting](troubleshooting.md) is
symptom-first and short, for use during an incident. The
[operations guide](operations.md) is the reference the other pages point at: it
lists every event and every reason. [Upgrading](upgrading.md) is the procedure,
and it assumes the operations guide for the rules behind it.

## Two things to know before you deploy

**Ownership before advertisement.** A node confirms its addresses in the kernel
before it advertises as their owner. If it cannot, it enters a fault state and
tells you which interface, which address, and which kernel error. It will not
announce an address it does not have.

**Reload is all-or-nothing.** A configuration change that cannot be fully
applied is refused in full, leaving the running configuration untouched.
Instances that the change does not affect are never interrupted.

## Current state

Highland is not yet released, and not yet finished. The state machine, the VRRPv3
codec, the configuration layer, the Linux netlink backend, the raw VRRP socket,
the control socket, the metrics endpoint, the event history, and the
transactional reload are all implemented and tested, and a whole failover —
including the announcement that tells the segment the address moved — is driven
end to end between two network namespaces in the test suite.

IPv6 and multicast are done as well: the same two-node suite runs IPv4 unicast,
IPv4 multicast, IPv6 unicast, and IPv6 multicast. What is not finished is holding
both families in one instance, health probes, and Keepalived interoperability.

The documentation describes the finished product, and marks anything that does
not work yet. The [command reference](cli.md), the
[troubleshooting guide](troubleshooting.md), and the
[changelog](../../CHANGELOG.md) are the reliable places to check. If a document
and the running software disagree, the running software wins — please report it.

## Reading the rule references

Configuration and event messages carry short references such as `V-01` or
`I-04`. They are stable, so you can quote one in a bug report and it will still
mean the same thing. Look them up in the [specification](../SPEC.md).

## Reporting problems

- A suspected vulnerability: `SECURITY.md`. Please do not open a public issue.
- Anything else: the project's issue tracker.
