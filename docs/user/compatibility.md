# Compatibility

## What Highland claims

- **Protocol compatibility.** Highland implements VRRPv3 and interoperates with
  standard implementations. This is tested by running it against Keepalived in
  two network namespaces, in both directions, rather than by asserting that two
  implementations that have only ever spoken to each other agree.
- **Not configuration compatibility.** Keepalived configuration is not
  supported directly. A documented subset can be imported, and the importer
  reports everything it could not translate.

Everything below is `1.0` work unless stated otherwise.

For the cutover procedure, the field mapping, and what to check at each step, see
the [migration guide](migration.md).

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

- **Timers.** Highland uses `3 * adver_int + Skew_Time` for the master-down
  interval, with `Skew_Time` as RFC 5798 §6.1 defines it,
  `((256 - priority) / 256) * adver_int`. At a one-second interval that is 3.41s
  behind a priority-150 master, not a fixed figure. Keepalived's practical
  behavior is the same.
- **Multicast TTL.** Must be 255. Highland refuses any other value (`V-24`), and
  sets `IP_MULTICAST_TTL` and `IPV6_MULTICAST_HOPS` explicitly: they are separate
  socket options from the unicast TTL and both default to 1, which every receiver
  discards.
- **The checksum.** RFC 5798 §5.2.8 covers the VRRP message **and** a pseudo-header
  whose next-header field is 112, for both address families, with the addresses in
  their own family's width. Highland originally skipped the pseudo-header for IPv4
  and could not hear Keepalived at all; the packet that settled it is a golden
  vector in the codec (`KEEPALIVED_V4_ADVERTISEMENT`).
- **Mixed families.** An instance is single-family (`V-03`). Split the
  configuration if you need both: one instance per family, with the addresses
  split between them.
- **IPv6 peers are link-local addresses.** RFC 5798 §5.1.2.1 makes the link-local
  address the source of every IPv6 advertisement, so a peer list naming a global
  address is an address nothing will ever hear from. The configuration reference
  says so where the key is described.
- **Preemption.** A master that sees a higher-priority advertisement steps down
  regardless of `preempt` (`R-16`). `preempt` only governs whether a backup may
  take over an existing master.
- **Degraded advertisement.** Highland never advertises master without owning
  the VIPs. There is no compatibility switch for this (`I-04`).
- **Unicast and multicast, IPv4 and IPv6, are tested against Keepalived**, in
  `crates/highland-daemon/tests/interop.rs`, in CI. Five of the six scenarios
  converge:

  | Scenario | Result |
  |---|---|
  | IPv4 unicast, Highland master | Converges; Keepalived stays a backup |
  | IPv4 unicast, Keepalived master | Converges, and the address is handed over on a kill |
  | IPv4 multicast | Converges, with `224.0.0.18` joined on both nodes |
  | IPv6 multicast | Converges, with `ff02::12` joined on both |
  | IPv6 unicast, Highland master | Converges; Keepalived stays a backup |
  | IPv6 unicast, Keepalived master | **Does not converge** — see below |

- **IPv6 unicast from Keepalived does not converge, and the reason is
  Keepalived's.** One line in your Highland configuration fixes it; see
  [the migration guide](migration.md#the-switch-allow_unconforming_hop_limit). Keepalived 2.3.3 advertises IPv6 unicast with a hop limit of
  **64**; its IPv6 multicast advertisements carry 255, which is why the multicast
  scenario passes. RFC 5798 §5.1.2.3 says a receiver MUST discard a packet whose
  hop limit is not 255, so Highland is right to discard it and an implementation
  that accepted it would be the bug. The test asserts the discard, with the value
  in the reason, rather than skipping the case.

## Capture-based diagnosis

1. Capture the segment: `tcpdump -i <iface> -w highland.pcap 'ip proto 112'`.
2. Decode with Wireshark's VRRP dissector, which handles both families.
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

IPv6 VIPs, IPv6 advertisements, unsolicited Neighbor Advertisements on takeover,
and IPv6 multicast over `ff02::12` all work today, and the two-node suite runs
the unicast and multicast cases side by side with their IPv4 equivalents. An
instance holds the addresses of one family, so a segment with both needs one
instance per family.

Two IPv6 details that differ from IPv4 and are worth knowing when reading a
capture:

- **The hop limit must be 255**, as on IPv4, and it is read from the packet
  rather than from the socket options.
- **The advertisement's checksum covers the IPv6 pseudo-header**, which includes
  the address the packet was sent to. A packet built for one destination and
  inspected as another will not verify, which is why a capture decoded by a
  third tool can disagree with a live node.
