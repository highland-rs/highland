# Migrating from Keepalived to Highland

This is the page to read before you put Highland next to a Keepalived node. It
covers what maps, what does not, how to find out which of your instances are
affected *before* you cut anything over, and how to reverse the change if the
result is not what you expected.

Everything in the "verified" column has been tested against Keepalived 2.3.3 in
two network namespaces, in both directions, by
`crates/highland-daemon/tests/interop.rs`. The test names are given so you can read
the assertion rather than take this page's word for it.

## First: which combinations interoperate?

| Family | Mode | Which node is the master | Works out of the box? |
|---|---|---|---|
| IPv4 | unicast | Highland | Yes |
| IPv4 | unicast | Keepalived | Yes |
| IPv4 | multicast | either | Yes |
| IPv6 | multicast | either | Yes |
| IPv6 | unicast | Highland | Yes |
| IPv6 | unicast | Keepalived | **No.** Needs the switch below |

The asymmetry in the last two rows is the whole of the problem, and it is worth
understanding before you plan the cutover.

**Highland as master works against any Keepalived node.** Keepalived accepts our
advertisements: they are version 3, with a hop limit of 255, from its configured
peer, with a checksum it verifies.

**Keepalived as master does not work against Highland over IPv6 unicast.** Keepalived
sends its IPv6 unicast advertisements with a hop limit of **64**. RFC 5798 §5.1.2.3
says a receiver MUST discard a packet whose hop limit is not 255, and Highland does.
Every advertisement is discarded, neither node times the other out quickly enough
to matter, and the practical result is that **Highland is not a working backup**:
it never takes over, and nothing in its log says why. The only clue is a
`bad_ttl` rejection counter.

This is Keepalived's behaviour and it is not configurable in 2.3.3: `hop_limit` is
rejected as an unknown keyword in both a `vrrp_instance` block and `global_defs`:

```console
$ keepalived -t -f /etc/keepalived/keepalived.conf
(/etc/keepalived/keepalived.conf: Line 8) Unknown keyword 'hop_limit'
```

So the fix is on your side, and it is one line.

## The switch: `allow_unconforming_hop_limit`

In the Highland instance that needs it:

```toml
[instance.network]
mode = "unicast"
peers = ["fe80::b822:1fff:fead:127e"]   # the peer's link-local, see below
allow_unconforming_hop_limit = true
```

It is **off by default**, and off means the hop limit is enforced.

### What it gives up

The hop limit of 255 is the only thing that proves an advertisement did not cross
a router: a packet that crossed one arrives with a smaller limit, and a value of
255 means it stayed on this link. With the switch on, Highland can no longer tell
that, so an advertisement that arrived from off this link would be believed. On a
flat layer-2 segment — which is what VRRP is for — the risk is low. On a routed
path, it is real, and you should not turn this on.

### What it does not do

It does not disable any other check. The TTL is the *only* rule this relaxes:
the version must still be 3, the VRID must still match, the checksum must still
verify against the source and destination, the source must still be a configured
peer or a member of the group, and a rate limit still applies.

### How you know it is on

The daemon logs it once, at startup:

```console
$ journalctl -u highland | grep 'hop limit'
WARN  highland_daemon::vrrp_transport: accepting advertisements whose hop limit is not
      255; the check that proves an advertisement stayed on this link is disabled
      for this instance instance=api interface="eth0"
```

If you do not see that line, the switch is not on and a Keepalived peer sending 64
is being discarded. That line is the difference between "I have relaxed a check
on purpose" and "I have relaxed a check and forgotten".

## IPv6 peers are link-local addresses

This trips up nearly every first migration, and it is not a Keepalived problem —
it is RFC 5798 §5.1.2.1, which says an IPv6 advertisement is sent from "the IPv6
link-local address of the interface the packet is being sent from".

```toml
# Highland with an IPv6 virtual address
[instance.network]
mode = "unicast"
peers = ["fe80::b822:1fff:fead:127e"]    # the peer's link-local, NOT its global address
```

A peer list naming a *global* IPv6 address names an address no advertisement will
ever arrive from, and the node sits as a backup that never hears its master. Find
the link-local address with:

```console
$ ip -6 addr show dev eth0 scope link
```

Do the same on the Keepalived side, and note that a Keepalived configuration for
IPv6 should **not** set `unicast_src_ip` to a global address: that is what makes
Keepalived relax the hop limit in the first place. Leave it unset and it uses the
link-local address, which is what the RFC names.

## Field-by-field mapping

| Keepalived | Highland | Notes |
|---|---|---|
| `vrrp_instance` | `[[instance]]` | |
| `interface` | `instance.interface` | |
| `virtual_router_id` | `instance.vrid` | `1..=255` |
| `priority` | `instance.priority` | `1..=255`; 255 is for the address owner |
| `advert_int` | `instance.advertisement_interval` | Durations need a unit: `"1s"` |
| `virtual_ipaddress` | `[[instance.vip]]` | CIDR, e.g. `192.0.2.100/24` |
| `unicast_peer` | `instance.network.peers` | Link-local addresses for IPv6 |
| `unicast_src_ip` | — | Chosen by the implementation; see above |
| `nopreempt` | `instance.preempt = false` | |
| `preempt_delay` | `instance.preempt_delay` | |
| `track_script` | `[[instance.check]]` | A native check, not a script |
| `notify_master` and friends | Event subscribers and the control API | |
| `authentication` | — | Not carried; see below |

Not mapped by design: `virtual_ipaddress` with `dev`, script security wrappers,
IPVS, and the LVS integration. VRRPv3 removed the authentication field, and
Keepalived says so itself when it sees one (`VRRP version 3 does not support
authentication. Ignoring.`); Highland has no equivalent, which is correct rather
than a gap.

## A cutover, one instance at a time

Do not convert both nodes at once. You want to be able to tell which change fixed
a problem.

1. **Check the new configuration before you touch either node.**

   ```console
   $ highland check-config /etc/highland/config.toml
   /etc/highland/config.toml is valid: 1 instance(s), schema version 1
   ```

2. **Bring Highland up as a *backup* while Keepalived keeps the address.** Nothing
   should move. If it does, you have a priority or address problem, not a
   migration problem.

3. **Check that Highland is hearing the advertisements.** This is the step people
   skip, and it is the one that catches the hop-limit and link-local problems
   before they matter:

   ```console
   $ highland events --follow
   ... node/api  advertisement received peer=fe80::b822:1fff:fead:127e priority=150
   ```

   Silence here means one of: the peer address is wrong (a global IPv6 address, or
   a typo), the hop limit is being discarded, or there is no peer list at all in
   multicast mode. If you have a peer named in the list that you cannot see
   advertised, compare the *discard* counters:

   ```console
   $ highland status --json | grep -A 20 rejected
   ```

   `bad_ttl` means the hop-limit case. `unknown_peer` means the source address is
   not the one you configured. `bad_checksum` means one of the two is computing
   the checksum over a different scope — see the interoperability notes if you see
   it at all, which should not happen with a current build.

4. **Lower Keepalived's priority below Highland's, and watch the address move.**
   Do it with `highland relinquish api --yes` on the Keepalived node if you would
   rather not edit its configuration during a window.

5. **Confirm the handover works by killing the master**, not by stopping it
   gracefully. A graceful stop gives back the address cleanly and proves nothing
   about a failure:

   ```console
   $ sudo kill -9 $(pidof highland)
   $ highland events --limit 20    # on the peer
   ```

   The address must appear on the peer within `Master_Down_Interval` — about
   3.4 seconds for a priority-150 node and 3.6 for a priority-100 one with a
   one-second interval.

6. **Confirm the announcement reached the segment.** Every client with the address
   in its cache must learn the new MAC, and that happens because Highland
   announces the takeover:

   ```console
   $ highland events --limit 50 | grep announce
   ```

   If clients keep sending to the dead node, the announcement is the problem, not
   the election.

## If it goes wrong

**Highland never becomes master.** It is hearing nothing, and the reason is a
rejection counter. `bad_ttl` on IPv6 with a Keepalived peer is the switch above;
`unknown_peer` is a wrong peer address.

**Both nodes hold the address.** Two daemons on one node, or a stale Keepalived
that was not actually stopped. Check for a second VRRP implementation before
debugging Highland.

**The address moves when nothing was wrong.** That is a health check or a
`preempt` setting, not a migration problem. A demoted master keeps advertising —
that is what RFC 5798 requires — and the backup only takes over if preemption is
enabled on the peer.

**Rollback.** Highland owns the address in the kernel. To hand it back, stop
Highland gracefully (`systemctl stop highland`, which sends a zero-priority
advertisement and removes the address) and let Keepalived take over. Do not
`kill -9` Highland and then start Keepalived: the address is still on the dead
node's interface, and the neighbour caches will point at it until they age out.

## What is verified, and how

| Claim | Test |
|---|---|
| IPv4 unicast, both directions | `highland_master_and_keepalived_backup_agree_on_one_master`, `keepalived_master_and_highland_backup_hand_over_when_it_dies` |
| IPv4 multicast | `highland_and_keepalived_agree_over_ipv4_multicast` |
| IPv6 multicast, and Keepalived's bytes re-encoded | `highland_and_keepalived_agree_over_ipv6_multicast` |
| IPv6 unicast, Highland master | `highland_master_and_keepalived_backup_agree_over_ipv6` |
| A hop limit of 64 is discarded by default | `an_ipv6_unicast_advertisement_with_the_wrong_hop_limit_is_discarded` |
| …and understood with the switch on, and the handover still works | `a_keepalived_that_sends_64_is_understood_when_the_switch_is_on` |
