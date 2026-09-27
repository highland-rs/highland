# Compatibility

## What Highland claims

- **Protocol compatibility.** Highland implements VRRPv3 and interoperates with
  standard implementations, verified by capture-based tests and, from Milestone 8,
  by running against Keepalived in isolated namespaces.
- **Not configuration compatibility.** Keepalived configuration is not
  supported directly. A documented subset can be imported, and the importer
  reports everything it could not translate.

Everything below is `1.0` work unless stated otherwise.

## Keepalived mapping

| Keepalived | Highland |
|---|---|
| `vrrp_instance` | `[[instance]]` |
| `interface` | `instance.interface` |
| `virtual_router_id` | `instance.vrid` |
| `priority` | `instance.priority` |
| `advert_int` | `instance.advertisement_interval` |
| `virtual_ipaddress` | `[[instance.vip]]` |
| `unicast_peer` | `instance.network.peers` |
| `nopreempt` | `instance.preempt = false` |
| `preempt_delay` | `instance.preempt_delay` |
| `track_script` | Native checks, or an explicit `command` check |
| `notify_master` and friends | Event subscribers and the control API |

Not mapped, by design: `virtual_ipaddress` with dev, script security wrappers,
IPVS, and the LVS integration.

## Differences operators should know

- **Timers.** Highland uses `3 * adver_int + 10ms` for the master-down interval,
  matching RFC 5798. Keepalived's practical behavior is the same.
- **Multicast TTL.** Must be 255. Highland refuses any other value (`V-24`).
- **Mixed families.** Before 1.0 an instance is single-family (`V-03`). Split
  the configuration if you need both.
- **Preemption.** A master that sees a higher-priority advertisement steps down
  regardless of `preempt` (`R-16`). `preempt` only governs whether a backup may
  take over an existing master.
- **Degraded advertisement.** Highland never advertises master without owning
  the VIPs. There is no compatibility switch for this (`I-04`).

## Capture-based diagnosis

1. Capture the segment: `tcpdump -i <iface> -w highland.pcap 'ip proto 112'`.
2. Decode with `highland` tooling as it lands, or with Wireshark's VRRP
   dissector.
3. Confirm for each advertisement: version 3, the expected VRID, priority, TTL
   255, the correct source address, and a checksum that matches.
4. A capture showing correct advertisements on the wire but a peer not
   transitioning points at host configuration, not the network.

## Split-brain behavior

Two masters can exist when both nodes can reach clients but not each other. This
is inherent to layer-2 failover. Highland detects and reports duplicate master
state and peer unreachability, and it does not pretend to prevent the condition.

Mitigations available to an operator:

- `preempt = false`, so a returning node does not take ownership
- `relinquish` during maintenance
- hold-down periods after a fault
- network design that keeps peers on a link that fails together

Fencing, external quorum, and cloud address-ownership adapters are post-1.0 and
deliberately outside the election algorithm.

## IPv6

From 1.0: IPv6 VIPs, IPv6 advertisements, unsolicited Neighbor Advertisements on
takeover, and IPv6 multicast. The state machine is family-agnostic; the
configuration model requires one family per instance before 1.0.
