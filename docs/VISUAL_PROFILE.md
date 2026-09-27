## Visual Identity: Highland

### 1. Brand Concept & Metaphor

* **The Metaphor:** The Scottish Highlands / Plateau. Immovable granite terrain, elevated ground ("high ground" / high availability), and structural resilience against harsh environments.
* **The Technical Core:** Virtual IP failover, VRRPv3 consensus, zero-cost memory safety, and active telemetry.
* **Tone:** Industrial, bedrock-stable, utilitarian, and uncompromisingly precise.

---

### 2. Logo & Mark Exploration

#### Concept A: The Fault-Tolerant Peak (Recommended)

* **Form:** Two parallel, angled vector strokes climbing upward like the slope of a mountain or dual redundant links. At the apex, one path seamlessly bridges over to a shared plateau (the virtual IP), while the other terminates gracefully without breaking the silhouette.
* **Geometric Foundation:** Built on a sharp 45° or 60° isometric grid, resembling both an elevation contour and a network topology node switch.
* **Why it works:** Works cleanly as a 16×16 favicon, a monochrome terminal glyph, and an SVG header badge.

#### Concept B: The Heartbeat Monolith

* **Form:** A solid hexagonal prism or chevron split cleanly down the center by a subtle pulse line (VRRP advertisement pulse). If one half dims, the bounding geometry remains unbroken.
* **Why it works:** Directly conveys the concept of dual-node state machine convergence and split-brain prevention.

---

### 3. Color Architecture

The palette pairs cold bedrock neutrals with functional network telemetry indicators.

| Role | Name | Hex | Usage |
| --- | --- | --- | --- |
| **Canvas / Bedrock** | Basalt Void | `#0D1117` | GitHub dark mode alignment, terminal background, README canvas |
| **Structure** | Granite Plate | `#1F2937` | Card backgrounds, structural borders, secondary containers |
| **Primary Accent** | Beacon Cyan | `#00E5A3` | "Master / Active" state, healthy metrics, active VIP highlight |
| **Rust / Memory Anchor** | Highland Rust | `#DE5B3E` | The nod to Rust's safety guarantees; used for highlights and CLI warnings |
| **Telemetry Amber** | Failover Drift | `#F59E0B` | State transitions, election states, backup nodes in standby |
| **Typography Base** | Frost Gray | `#E5E7EB` | Primary body text, crisp mono readouts |

---

### 4. Typography System

* **Logotype:** **JetBrains Mono** or **Space Grotesk** (SemiBold / All-Caps, wide tracking `+0.12em`).
* *Rendering:* `HIGHLAND` with a muted version string or RFC badge (`VRRPv3 / RFC 5798`).


* **Documentation & Readme Headers:** **Inter** or **Geist Sans** (Clean, neutral geometric sans for dense architectural prose).
* **CLI & Telemetry Display:** **Berkeley Mono** or **JetBrains Mono** (Strict tabular lining figures for interface stats, priority numbers, and millisecond failover timers).

---

### 5. Repository Assets & README Layout

#### Header Hero

A minimal, terminal-inspired SVG banner with a dark slate background (`#0D1117`), featuring:

* Monospaced ASCII/vector grid topology showing Master (`MASTER: 10.0.0.1`) and Backup (`BACKUP: 10.0.0.2`) converging on `VIP: 10.0.0.254`.
* Clean status pills:
* `[ safe: #![forbid(unsafe_code)] ]`
* `[ spec: RFC 5798 / VRRPv3 ]`
* `[ target: Linux / netlink ]`



#### Terminal / Log Stoppage Aesthetic

Highland’s runtime output should carry the same visual identity. Rather than raw unstructured prints, format stdout/tracing logs with strict alignment:

```text
[2026-09-27T11:21:00Z INFO  highland::state] INIT -> BACKUP interface=eth0 vrid=51 priority=100
[2026-09-27T11:21:03Z WARN  highland::vrrp]  master timer expired, asserting election
[2026-09-27T11:21:03Z INFO  highland::state] BACKUP -> MASTER vrid=51 vip=192.168.1.1/24
[2026-09-27T11:21:03Z INFO  highland::netlink] gratuitous ARP broadcast sent (3 frames)

```

---

### 6. Badges & Micro-Assets

For crate documentation and GitHub releases, use flat-square badges (`style=flat-square`):

* **Engine:** `Rust 2024 / MSRV 1.85+` (Rust Orange `#DE5B3E`)
* **Transport:** `Raw Sockets / Netlink` (Granite `#1F2937`)
* **State Machine:** `Deterministic VRRPv3` (Beacon Cyan `#00E5A3`)
* **Observability:** `Tracing + Prometheus` (Purple `#8B5CF6`)
