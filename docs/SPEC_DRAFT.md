# Highland: A Rust-Native VRRP and Linux VIP Failover Crate

## 1. Project Summary

**Highland** is a memory-safe, observable, Linux-focused Rust implementation of high-availability virtual IP failover.

Its initial purpose is to provide a modern alternative to the VRRP portion of Keepalived, with particular emphasis on:

- Correct VRRPv3 behavior
- IPv4 and IPv6 support
- Unicast peer communication
- Deterministic state transitions
- Native health checks
- Split-brain resistance
- Structured observability
- Strong configuration validation
- Testability through network namespaces and simulation
- A reusable Rust library rather than only a daemon

Highland should begin as a Rust crate with a production daemon built on top:

```text
highland/
├── highland-core       # State machines, policies, timers, domain types
├── highland-vrrp       # VRRP packet encoding, decoding, validation
├── highland-net        # Linux interface, address, route, ARP/ND operations
├── highland-checks     # Health-check implementations
├── highland-config     # Configuration types, parsing, validation
├── highland-observe    # Logging, metrics, event stream
├── highland-control    # Local control API
├── highland-daemon     # Main executable
└── highland-cli        # Administrative command-line interface
```

The project must not initially attempt to replicate all of Keepalived. It should focus on becoming an excellent VRRP/VIP failover system with a clean extension path.

---

## 2. Product Positioning

Highland is not merely “VRRP written in Rust.” Its primary value proposition is:

> A declarative, explainable, testable, and memory-safe Linux failover controller for virtual IP ownership.

The project should improve on traditional designs by providing:

- Native checks instead of requiring shell scripts for common cases
- Explicit and inspectable role transitions
- First-class metrics and event history
- Safer configuration semantics
- Testable state-machine behavior independent of Linux networking
- A library API usable by other Rust infrastructure projects
- Support for both traditional multicast VRRP deployments and cloud-friendly unicast deployments

Highland should be compatible with the VRRP protocol, but its internal API should not be constrained by historical configuration or implementation decisions.

---

## 3. Goals

### 3.1 Primary goals

Highland must:

1. Implement VRRPv3 for IPv4 and IPv6.
2. Support one or more independent VRRP instances.
3. Manage virtual IPv4 and IPv6 addresses on Linux interfaces.
4. Support multicast and unicast peer modes.
5. Perform deterministic MASTER, BACKUP, and INIT transitions.
6. Support priority-based master election.
7. Send and process advertisements correctly.
8. Detect master failure within protocol-defined timing constraints.
9. Add and remove VIPs safely.
10. Support native health checks.
11. Expose structured logs and Prometheus metrics.
12. Provide a local administrative/control interface.
13. Operate without shelling out for core functionality.
14. Be testable in isolated network namespaces.
15. Provide clear failure reasons and transition explanations.
16. Support graceful shutdown and relinquishing of VIP ownership.
17. Remain usable in minimal Linux environments.

### 3.2 Secondary goals

Highland should eventually:

- Import a useful subset of Keepalived configuration.
- Support BFD.
- Support dynamic peer discovery in controlled environments.
- Support cloud-provider floating-IP adapters.
- Support optional IPVS integration.
- Support capability-based privilege separation.
- Provide a simulation and replay mode.
- Support an embedded library use case without requiring the daemon.
- Offer a stable C ABI only if there is demonstrated demand.

---

## 4. Non-Goals

The first major release must not attempt to:

- Implement every Keepalived feature.
- Implement IPVS load balancing.
- Replace Pacemaker or Corosync.
- Manage arbitrary routing protocols.
- Implement a general-purpose service supervisor.
- Execute arbitrary shell scripts by default.
- Support non-Linux operating systems in the initial version.
- Promise wire-level compatibility with every Keepalived-specific extension.
- Modify firewall rules automatically.
- Manage cloud provider APIs in the core crate.
- Provide automatic cluster membership or consensus beyond VRRP.
- Guarantee protection against every possible network partition.
- Hide fundamental split-brain limitations of layer-2 failover.

The initial product should be a high-quality VRRP/VIP controller, not a complete infrastructure orchestration platform.

---

## 5. Terminology

| Term | Meaning |
|---|---|
| VRRP instance | One logical VRRP group with a VRID, priority, interface, and VIP set |
| VRID | Virtual Router Identifier, ranging from 1 to 255 |
| MASTER | Node currently responsible for owning the VIP set and sending advertisements |
| BACKUP | Node monitoring advertisements and eligible to become MASTER |
| INIT | Startup or uninitialized state before normal participation |
| VIP | Virtual IP address managed by a VRRP instance |
| Advertisement | VRRP packet announcing the current MASTER and its priority |
| Owner | The node whose physical address corresponds to the configured virtual router identity, if applicable |
| Effective priority | Configured priority adjusted by health-check results |
| Preemption | Behavior where a higher-priority BACKUP takes ownership from a lower-priority MASTER |
| Preemption delay | Configured delay before a higher-priority node takes ownership |
| Unicast peer | Explicit VRRP peer address used instead of multicast |
| Split brain | Multiple nodes independently believing they are MASTER |
| Track item | Health or system condition that modifies eligibility or priority |
| Graceful relinquish | Intentional transition from MASTER to BACKUP while removing VIPs |

---

## 6. Supported Deployment Model

### 6.1 Initial platform

The first release targets:

- Linux
- `x86_64`
- `aarch64`
- Network namespaces for tests
- systemd-based and non-systemd environments

The implementation should avoid unnecessary dependencies on systemd. systemd integration may be provided through unit files and optional readiness notification, but the core daemon must work without systemd.

### 6.2 Required privileges

The daemon needs privileges to:

- Open raw or packet sockets as required by VRRP mode
- Add and remove addresses
- Inspect interfaces
- Receive link-state notifications
- Set or inspect relevant link properties

The implementation should support privilege reduction after initialization where practical.

Potential deployment strategies:

1. Run as root initially.
2. Add Linux capabilities documentation.
3. Support a privileged networking helper in a later release.
4. Avoid requiring unrestricted shell execution.

---

## 7. Workspace Structure

```text
highland/
├── Cargo.toml
├── Cargo.lock
├── LICENSE-APACHE
├── LICENSE-MIT
├── README.md
├── CONTRIBUTING.md
├── SECURITY.md
├── CHANGELOG.md
├── docs/
│   ├── architecture.md
│   ├── configuration.md
│   ├── operations.md
│   ├── threat-model.md
│   ├── compatibility.md
│   └── testing.md
├── crates/
│   ├── highland-core/
│   ├── highland-vrrp/
│   ├── highland-net/
│   ├── highland-checks/
│   ├── highland-config/
│   ├── highland-observe/
│   ├── highland-control/
│   ├── highland-daemon/
│   └── highland-cli/
├── tests/
│   ├── unit/
│   ├── integration/
│   ├── network-ns/
│   ├── packet-captures/
│   └── compatibility/
├── fuzz/
│   ├── vrrp-packet/
│   ├── config-parser/
│   ├── health-response/
│   └── netlink-event/
└── deploy/
    ├── systemd/
    ├── openrc/
    └── containers/
```

---

## 8. Crate Responsibilities

## 8.1 `highland-core`

Pure Rust domain logic with no Linux-specific code.

Responsibilities:

- VRRP instance state machine
- Election logic
- Advertisement timer calculations
- Health-weight calculations
- Preemption logic
- Transition reasons
- Event types
- Clock abstraction
- Randomness abstraction where needed
- Configuration-independent policy evaluation

This crate must be usable in deterministic unit tests.

It must not:

- Open sockets
- Modify interfaces
- Read files
- Spawn processes
- Depend on Linux
- Perform logging directly

### Suggested public modules

```rust
pub mod election;
pub mod health;
pub mod state;
pub mod timer;
pub mod transition;
pub mod types;
pub mod clock;
```

---

## 8.2 `highland-vrrp`

Protocol implementation.

Responsibilities:

- VRRPv3 packet encoding
- VRRPv3 packet decoding
- Checksum calculation
- IPv4 and IPv6 packet validation
- Advertisement parsing
- Authentication-policy handling if applicable
- TTL/hop-limit validation
- Source-address validation
- Peer filtering
- Packet size and field validation

The protocol crate should expose packet operations independently from sockets.

Example API:

```rust
pub struct Advertisement {
    pub version: Version,
    pub vrid: Vrid,
    pub priority: Priority,
    pub advert_interval: Duration,
    pub addresses: Vec<IpAddr>,
}

pub fn encode_advertisement(
    advertisement: &Advertisement,
    family: IpFamily,
) -> Result<Vec<u8>, EncodeError>;

pub fn decode_advertisement(
    bytes: &[u8],
    family: IpFamily,
) -> Result<Advertisement, DecodeError>;
```

The decoder must reject:

- Truncated packets
- Unsupported versions
- Invalid VRID values
- Invalid priority values
- Invalid address counts
- Incorrect checksums
- Unsupported address families
- Malformed lengths
- Unexpected packet types
- Packets violating configured peer rules

The parser must never panic on untrusted network input.

---

## 8.3 `highland-net`

Linux networking integration.

Responsibilities:

- Interface discovery
- Interface up/down status
- Link-state monitoring
- Address addition/removal
- Route inspection where necessary
- ARP and Neighbor Advertisement operations
- Netlink interaction
- Raw packet or socket handling
- Interface index resolution
- Namespace-aware operations

The crate should use typed wrappers around low-level Linux concepts.

Potential internal abstractions:

```rust
pub trait NetworkBackend {
    fn interface(&self, name: &str) -> Result<Interface, NetError>;
    fn add_address(&self, interface: InterfaceId, address: IpCidr) -> Result<(), NetError>;
    fn remove_address(&self, interface: InterfaceId, address: IpCidr) -> Result<(), NetError>;
    fn send_gratuitous_update(&self, interface: InterfaceId, address: IpAddr)
        -> Result<(), NetError>;
}
```

Production Linux implementations may use Netlink directly or a carefully selected Rust crate. The choice must be documented and tested.

---

## 8.4 `highland-checks`

Native health checking.

Initial checks:

- TCP connect
- HTTP/HTTPS readiness request
- DNS query
- Process existence
- Unix socket connect
- Interface/link state
- File existence
- Local command execution, disabled by default
- Composite checks

Each check must produce a structured result:

```rust
pub struct CheckResult {
    pub status: CheckStatus,
    pub latency: Option<Duration>,
    pub reason: String,
    pub observed_at: Instant,
}
```

Possible statuses:

```rust
pub enum CheckStatus {
    Passing,
    Failing,
    Unknown,
    TimedOut,
    Disabled,
}
```

Checks must support:

- Timeout
- Initial grace period
- Consecutive failure threshold
- Consecutive success threshold
- Retry interval
- Weight
- Fail-open or fail-closed policy
- Per-check metrics
- Cancellation
- Explicit resource limits

Arbitrary command execution should be isolated in a separate feature and clearly marked as unsafe operational behavior.

---

## 8.5 `highland-config`

Configuration model, parser, validation, and redaction.

Recommended initial format:

- TOML for human-authored configuration
- JSON output for inspection
- Optional YAML later, if justified

The configuration must separate:

1. Node-level settings
2. Instance-level settings
3. Network settings
4. Health policy
5. Observability
6. Security and privilege settings

Configuration loading must support:

- File parsing
- Environment-variable substitution only when explicitly enabled
- Schema validation
- Semantic validation
- Default values
- Clear source locations for errors
- Redacted display
- Atomic reload preparation

Invalid reloads must not affect the running configuration.

---

## 8.6 `highland-observe`

Observability primitives.

Responsibilities:

- Structured tracing
- Metrics definitions
- Event stream
- State snapshots
- Optional OpenTelemetry integration
- Log redaction
- Transition explanations

The daemon should expose:

- Logs to stderr or journald
- Prometheus metrics endpoint
- Unix-socket status API
- Optional JSON event stream

---

## 8.7 `highland-control`

Local administrative API.

Initial transport:

- Unix domain socket
- File permissions and group ownership
- Optional peer credential verification

Operations:

```text
status
status --json
instances
show <instance>
reload
pause <instance>
resume <instance>
relinquish <instance>
force-transition <instance>   # disabled by default
events
health
```

Dangerous operations must require explicit flags and produce audit events.

The API must never provide a remote unauthenticated control interface by default.

---

## 8.8 `highland-daemon`

Production executable.

Responsibilities:

- Parse command-line arguments
- Load configuration
- Initialize logging and metrics
- Create runtime components
- Start health checks
- Start protocol listeners
- Coordinate state machines
- Apply network ownership changes
- Handle signals
- Perform graceful shutdown
- Execute configuration reloads

The daemon must use structured concurrency. Tasks should have explicit ownership and cancellation behavior.

---

## 8.9 `highland-cli`

Administrative CLI.

Example commands:

```text
highland check-config /etc/highland/config.toml
highland run --config /etc/highland/config.toml
highland status
highland status --json
highland reload
highland relinquish api
highland events --follow
highland simulate --config ./example.toml
```

The CLI should connect to the control socket where possible rather than duplicating daemon logic.

---

## 9. Configuration Specification

Example:

```toml
[node]
name = "node-a"
instance_id = "node-a"

[logging]
level = "info"
format = "json"

[metrics]
enabled = true
listen = "127.0.0.1:9900"

[control]
socket = "/run/highland/control.sock"
group = "highland"

[[instance]]
name = "api"
interface = "eth0"
vrid = 42
priority = 150
advertisement_interval = "1s"
preempt = false
preempt_delay = "30s"
startup_delay = "5s"

[instance.network]
mode = "unicast"
peers = [
    "192.0.2.11",
    "192.0.2.12",
]

[[instance.vip]]
address = "192.0.2.10/24"

[[instance.vip]]
address = "2001:db8:10::10/64"

[instance.health]
failure_policy = "degrade"
minimum_effective_priority = 100
all_checks_required = true

[[instance.check]]
name = "api-ready"
type = "http"
url = "http://127.0.0.1:8080/ready"
method = "GET"
timeout = "500ms"
interval = "1s"
failure_threshold = 3
success_threshold = 2
weight = 100
expected_status = [200]

[[instance.check]]
name = "database-port"
type = "tcp"
address = "127.0.0.1:5432"
timeout = "300ms"
interval = "2s"
failure_threshold = 2
weight = 50
```

### 9.1 Required validation rules

Reject configurations when:

- VRID is outside 1–255.
- Priority is outside 1–254 unless the protocol explicitly permits a special value.
- A VIP family is incompatible with the selected instance/interface.
- Duplicate instance names exist.
- Duplicate VRIDs conflict on the same interface and network context.
- A unicast peer is unspecified in unicast mode.
- The local node is configured as its own peer.
- Advertisement interval is invalid or outside supported bounds.
- A check has a zero or negative timeout.
- A check has an impossible threshold.
- An instance has no VIPs.
- A VIP appears in multiple conflicting instances.
- `preempt_delay` is used without preemption.
- Unsafe command execution is enabled without explicit configuration.
- A requested interface does not exist, unless deferred interface binding is enabled.

### 9.2 Reload semantics

On reload:

1. Parse the new configuration.
2. Validate syntax.
3. Validate semantics.
4. Compute a change plan.
5. Display or log the planned changes.
6. Apply changes transactionally where possible.
7. Preserve unaffected instances.
8. Remove VIPs only when required.
9. Reject the entire reload if a critical change cannot be applied safely.

The daemon must not silently convert a malformed configuration into a partial runtime state.

---

## 10. VRRP State Machine

The core state machine should be explicit and independently testable.

### 10.1 States

```rust
pub enum Role {
    Init,
    Backup,
    Master,
    Fault,
    Disabled,
}
```

### 10.2 Events

```rust
pub enum Event {
    Startup,
    InterfaceUp,
    InterfaceDown,
    AdvertisementReceived(Advertisement),
    AdvertisementTimeout,
    HealthChanged(HealthSummary),
    ShutdownRequested,
    ConfigurationReloaded,
    PeerBecameUnreachable,
    PeerBecameReachable,
    TimerExpired(TimerId),
    NetworkOperationFailed(NetworkError),
}
```

### 10.3 Transition output

The state machine should not directly modify the network. It should emit commands:

```rust
pub enum Action {
    StartAdvertisementTimer,
    StopAdvertisementTimer,
    StartMasterDownTimer,
    StopMasterDownTimer,
    BecomeMaster,
    BecomeBackup,
    SendAdvertisement,
    AddVirtualAddresses,
    RemoveVirtualAddresses,
    SendGratuitousUpdates,
    EmitEvent(TransitionEvent),
    EnterFault(String),
}
```

This separation allows deterministic testing.

### 10.4 Transition invariants

The following must always hold:

- BACKUP must not own VIPs.
- MASTER must own all configured VIPs unless in a transitional error state.
- INIT must not send normal advertisements.
- FAULT must not claim ownership.
- A node must not become MASTER while the interface is down.
- A node must not accept advertisements from an invalid peer.
- A node must not remove another instance’s VIPs.
- Repeated advertisements must reset the correct timer.
- Graceful relinquish must remove ownership before process exit where possible.
- Every role transition must have a structured reason.

---

## 11. Election and Priority Model

Highland should distinguish:

- Configured priority
- Health-adjusted priority
- Effective priority
- Eligibility
- Preemption policy

Suggested model:

```rust
pub struct PriorityState {
    pub configured: u8,
    pub health_penalty: u8,
    pub effective: u8,
    pub eligible: bool,
}
```

### 11.1 Health policies

Support at least:

- `fail_closed`: any critical failure makes the node ineligible.
- `degrade`: health failures reduce priority.
- `weighted`: checks contribute configurable penalties.
- `manual`: health does not affect election unless explicitly changed.

### 11.2 Tie-breaking

Tie-breaking must be deterministic and protocol-compliant. The implementation must document:

- Whether address comparison is used.
- Which address family is preferred if both are configured.
- How unicast peers are ordered.
- How equal-priority candidates are handled.

### 11.3 Preemption

Support:

- Enabled/disabled preemption
- Preemption delay
- Immediate takeover on master failure
- Delayed takeover after recovery
- No-preempt operation for cloud or operational environments

A node should not repeatedly oscillate between roles because of short-lived health failures.

---

## 12. Networking Requirements

### 12.1 Address ownership

When becoming MASTER:

1. Verify interface availability.
2. Verify the address is not already owned by another local interface.
3. Add configured VIPs.
4. Confirm addresses appear in the kernel.
5. Send gratuitous ARP for IPv4.
6. Send unsolicited Neighbor Advertisements for IPv6.
7. Begin normal advertisements.
8. Emit a successful ownership event.

If any critical step fails:

- Do not claim successful MASTER state.
- Enter a controlled failure state.
- Retry according to policy.
- Emit an actionable error.

When becoming BACKUP:

1. Stop normal advertisements.
2. Remove VIPs.
3. Confirm removal.
4. Emit a relinquish event.

### 12.2 Multicast mode

The implementation must support standard VRRP multicast behavior appropriate to IPv4 and IPv6.

Required considerations:

- Correct multicast group
- Correct protocol number
- Correct TTL/hop limit
- Interface binding
- Source address selection
- Receive filtering
- Firewall diagnostics
- Multicast membership lifecycle

### 12.3 Unicast mode

Unicast mode must support:

- Explicit peer list
- Per-peer send behavior
- Peer allow-list validation
- Peer health visibility
- Duplicate packet suppression where necessary
- IPv4 and IPv6 peers
- Interface-bound sockets
- Clear behavior when one peer fails but others remain reachable

### 12.4 Network namespaces

The code must be namespace-aware enough for integration tests. The test harness should create:

```text
node-a namespace ─ veth ─ bridge ─ veth ─ node-b namespace
```

Tests must be able to simulate:

- Link failure
- Packet loss
- Packet delay
- Partition
- Interface restart
- Address conflicts
- Concurrent startup
- Asymmetric reachability

---

## 13. Health Checks

### 13.1 Native checks

Initial implementations:

#### TCP

```toml
[[instance.check]]
name = "postgres"
type = "tcp"
address = "127.0.0.1:5432"
timeout = "500ms"
interval = "2s"
```

#### HTTP

```toml
[[instance.check]]
name = "api"
type = "http"
url = "http://127.0.0.1:8080/healthz"
timeout = "1s"
expected_status = [200]
```

#### HTTPS

Must support:

- Certificate validation by default
- Configurable trust roots
- Explicit insecure mode
- SNI
- Timeout
- Response-size limit

#### DNS

Support querying:

- A
- AAAA
- SRV
- TXT where useful

#### Unix socket

Useful for local services and daemons.

#### Process

Process existence should be treated as a weak signal. It must not imply readiness.

#### Interface

Check:

- Link present
- Link up
- Carrier state
- Optional address presence

### 13.2 Check aggregation

Each check has:

- Name
- Interval
- Timeout
- Failure threshold
- Recovery threshold
- Weight
- Criticality
- Initial grace period

Example:

```rust
pub enum CheckCriticality {
    Advisory,
    Weighted,
    Critical,
}
```

### 13.3 Hysteresis

Health changes must be debounced.

A check should not immediately demote a node because of one transient timeout unless explicitly configured to do so.

The event stream must distinguish:

- Single failed probe
- Check entered failing state
- Check recovered
- Instance became ineligible
- Instance became eligible

---

## 14. Observability

### 14.1 Structured events

Every important event should have:

- Timestamp
- Node name
- Instance name
- Previous state
- New state
- Reason
- Peer information if applicable
- Effective priority
- Health summary
- Network operation result

Example:

```json
{
  "event": "role_transition",
  "instance": "api",
  "from": "MASTER",
  "to": "BACKUP",
  "reason": "higher_priority_master_advertisement",
  "peer": "192.0.2.11",
  "local_priority": 120,
  "remote_priority": 150,
  "timestamp": "2027-01-03T12:00:14.123Z"
}
```

### 14.2 Metrics

At minimum:

```text
highland_instance_role{instance="api"} 1
highland_instance_effective_priority{instance="api"} 150
highland_instance_health{instance="api"} 1
highland_instance_transitions_total{instance="api",from="BACKUP",to="MASTER"} 2
highland_advertisements_sent_total{instance="api"} 1000
highland_advertisements_received_total{instance="api"} 1200
highland_invalid_packets_total{instance="api",reason="checksum"} 4
highland_master_down_events_total{instance="api"} 1
highland_vip_add_failures_total{instance="api"} 0
highland_vip_remove_failures_total{instance="api"} 0
highland_check_failures_total{instance="api",check="api-ready"} 3
highland_check_latency_seconds{instance="api",check="api-ready"} 0.012
```

Avoid high-cardinality labels such as arbitrary peer addresses unless explicitly enabled.

### 14.3 Status API

The status response should include:

```json
{
  "node": "node-a",
  "instances": [
    {
      "name": "api",
      "role": "MASTER",
      "vrid": 42,
      "priority": 150,
      "effective_priority": 150,
      "vip_addresses": ["192.0.2.10/24"],
      "health": "healthy",
      "master_down_timer": null,
      "last_advertisement": "2027-01-03T12:00:14Z",
      "checks": {
        "api-ready": "passing",
        "database-port": "passing"
      }
    }
  ]
}
```

---

## 15. Split-Brain and Failure Handling

Highland must explicitly document that VRRP cannot solve every split-brain scenario.

### 15.1 Detection and mitigation

Support:

- Peer reachability state
- Duplicate MASTER detection
- Unexpected advertisement-source reporting
- Optional conflict logging
- Gratuitous ARP conflict observation where available
- Hold-down periods
- No-preempt mode
- Operator-triggered relinquish
- Optional external fencing hooks in a later release

### 15.2 Fencing

Fencing must not be implied by VRRP.

Future integrations may include:

- Cloud provider address ownership
- IPMI
- Redfish
- Watchdog devices
- External quorum services
- Cluster managers

These should be separate crates or adapters, not part of the core election algorithm.

### 15.3 Network partition behavior

The documentation must define expected behavior for:

- Both nodes can reach clients but not each other
- One node can reach the peer but not clients
- One node loses only multicast
- Unicast peers become asymmetric
- An old MASTER resumes after a long pause
- A paused process resumes after timers have expired

---

## 16. Security Requirements

### 16.1 Input handling

All network input is untrusted.

Requirements:

- No parser panics
- Bounded packet sizes
- Bounded address counts
- Bounded event queues
- Bounded HTTP response bodies
- Bounded command output
- Timeout every network operation
- Avoid unbounded allocations from configuration or packets

### 16.2 Configuration security

- Refuse world-writable configuration files where practical.
- Do not log secrets.
- Avoid secrets in process arguments.
- Validate control-socket permissions.
- Make command execution opt-in.
- Restrict command paths if command checks are enabled.
- Prevent path traversal in any future file-based checks.

### 16.3 Privilege reduction

Document required Linux capabilities.

Potential future model:

```text
highland-supervisor
├── highland-protocol
├── highland-checks
└── highland-net-helper
```

The networking helper would receive narrow commands over a local authenticated channel.

### 16.4 Denial-of-service resistance

Protect against:

- Advertisement floods
- Invalid packet floods
- Repeated state transitions
- Health-check amplification
- Huge configuration files
- Excessive peer lists
- Slow HTTP responses
- Repeated reload requests

---

## 17. Error Model

Errors should be typed and actionable.

Example:

```rust
pub enum HighlandError {
    Config(ConfigError),
    Protocol(ProtocolError),
    Network(NetworkError),
    Check(CheckError),
    Control(ControlError),
    Shutdown(ShutdownError),
}
```

Errors should preserve context:

```rust
NetworkError::AddAddress {
    interface: String,
    address: IpCidr,
    source: io::Error,
}
```

CLI output should explain:

```text
failed to become MASTER for instance "api":
could not add VIP 192.0.2.10/24 to interface eth0:
Operation not permitted

Check:
  - process has CAP_NET_ADMIN
  - interface exists
  - address is not already assigned
```

---

## 18. Testing Strategy

Testing is a core feature of the project, not a later task.

## 18.1 Unit tests

Test:

- Packet encoding and decoding
- Checksum calculation
- Advertisement interval calculations
- Master-down timer calculations
- Priority comparison
- Preemption behavior
- Health aggregation
- State transitions
- Configuration validation
- Reload planning
- Event generation

Use deterministic fake clocks.

## 18.2 Property tests

Use property-based testing for:

- Packet round trips
- Arbitrary valid advertisements
- Invalid-length handling
- Priority calculations
- State machine invariants
- Configuration normalization

## 18.3 Fuzzing

Required fuzz targets:

```text
fuzz_vrrp_ipv4_packet
fuzz_vrrp_ipv6_packet
fuzz_config_document
fuzz_check_response
fuzz_netlink_message
fuzz_control_request
```

Every fuzz target must have:

- A bounded input
- No network access
- No filesystem access unless explicitly needed
- A regression corpus
- CI integration

## 18.4 Network namespace integration tests

Tests should create isolated topologies and verify:

1. Node A becomes MASTER.
2. Node B remains BACKUP.
3. Node A fails.
4. Node B becomes MASTER.
5. VIP moves correctly.
6. Gratuitous ARP/ND behavior occurs.
7. Node A returns.
8. Preemption policy is honored.
9. Health failure causes expected demotion.
10. Packet loss does not cause unnecessary oscillation.
11. Concurrent startup resolves deterministically.
12. Configuration reload preserves unaffected instances.

## 18.5 Chaos tests

Simulate:

- Dropped advertisements
- Delayed advertisements
- Duplicated advertisements
- Reordered packets
- Link flaps
- Address-add failures
- Address-remove failures
- Process pauses
- Clock jumps
- Slow health checks
- Stale sockets
- Kernel netlink errors
- Simultaneous MASTER election

## 18.6 Compatibility tests

Where practical, run Highland and Keepalived in isolated namespaces and verify:

- Highland MASTER / Keepalived BACKUP
- Keepalived MASTER / Highland BACKUP
- IPv4 multicast
- IPv6 multicast
- Unicast mode
- Priority changes
- Preemption
- Graceful shutdown
- Advertisement interval handling

The compatibility target must be documented as protocol compatibility, not complete configuration compatibility.

---

## 19. Correctness Invariants

The implementation should encode and test these invariants:

1. At most one local role is active per instance.
2. BACKUP never intentionally owns the VIP.
3. INIT never transmits normal advertisements.
4. A node cannot become MASTER while its interface is unavailable.
5. A node cannot advertise MASTER without successful VIP ownership unless explicitly configured for degraded advertisement behavior.
6. Every ownership change has a corresponding event.
7. Every timer has a cancellation path.
8. Health checks cannot block the protocol loop.
9. A malformed packet cannot terminate the daemon.
10. Reload failure cannot corrupt the active configuration.
11. Shutdown is idempotent.
12. Repeated network errors do not cause unbounded retries or memory growth.
13. A stale health result cannot overwrite a newer result.
14. A stale configuration-generation result cannot modify a newer instance.
15. No async task may outlive the instance that owns it without explicit cancellation.

---

## 20. Async Runtime and Concurrency

The implementation should use a single async runtime consistently.

Possible choices:

- Tokio
- smol
- Another well-supported runtime

The decision should be made during planning based on:

- Timer precision
- Linux socket support
- Cancellation behavior
- Runtime footprint
- Ecosystem compatibility
- Test-time determinism

Recommended architecture:

```text
supervisor
├── configuration task
├── control API task
├── metrics task
├── per-instance task
│   ├── VRRP receive loop
│   ├── advertisement timer
│   ├── master-down timer
│   ├── health-check coordinator
│   └── network ownership coordinator
└── shutdown coordinator
```

Avoid a shared mutable global state model. Each instance should have an owned actor/task or an explicit event loop.

The state machine itself should remain synchronous and deterministic. Async code should adapt external events into state-machine events.

---

## 21. State Machine Event Loop

Conceptual loop:

```rust
loop {
    tokio::select! {
        event = protocol_rx.recv() => {
            let actions = state_machine.handle(event?);
            executor.apply(actions).await?;
        }

        event = health_rx.recv() => {
            let actions = state_machine.handle(event?);
            executor.apply(actions).await?;
        }

        event = timer_rx.recv() => {
            let actions = state_machine.handle(event?);
            executor.apply(actions).await?;
        }

        event = control_rx.recv() => {
            let actions = state_machine.handle(event?);
            executor.apply(actions).await?;
        }

        _ = shutdown.cancelled() => {
            let actions = state_machine.handle(Event::ShutdownRequested);
            executor.apply(actions).await?;
            break;
        }
    }
}
```

The executor must handle partial failure and report results back to the state machine rather than assuming network operations always succeed.

---

## 22. Command-Line Interface

### 22.1 Commands

```text
highland run
highland check-config
highland status
highland status --json
highland instances
highland show <instance>
highland reload
highland relinquish <instance>
highland pause <instance>
highland resume <instance>
highland events
highland simulate
highland version
```

### 22.2 Safety behavior

Commands that can remove VIPs or alter role state must:

- Require the control socket.
- Display the target instance.
- Show the current role.
- Require explicit confirmation for destructive actions unless `--yes` is used.
- Emit an audit event.

---

## 23. Systemd Integration

Provide an example unit:

```ini
[Unit]
Description=Highland VRRP failover daemon
After=network-online.target
Wants=network-online.target
Before=keepalived.service

[Service]
Type=notify
ExecStart=/usr/bin/highland run --config /etc/highland/config.toml
Restart=on-failure
RestartSec=2s
AmbientCapabilities=CAP_NET_ADMIN CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_RAW
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
RuntimeDirectory=highland
StateDirectory=highland
ConfigurationDirectory=highland

[Install]
WantedBy=multi-user.target
```

The unit must be treated as a starting point, not universally correct. Documentation must explain capability and namespace requirements.

---

## 24. Keepalived Compatibility Strategy

Compatibility should be incremental.

### Phase 1: Protocol compatibility

- Interoperate with standard VRRP peers.
- Validate against packet captures.
- Support standard IPv4 and IPv6 behavior.

### Phase 2: Configuration mapping

Support a documented subset of common Keepalived concepts:

| Keepalived concept | Highland equivalent |
|---|---|
| `vrrp_instance` | `[[instance]]` |
| `interface` | `interface` |
| `virtual_router_id` | `vrid` |
| `priority` | `priority` |
| `advert_int` | `advertisement_interval` |
| `virtual_ipaddress` | `[[instance.vip]]` |
| `unicast_peer` | `network.peers` |
| `nopreempt` | `preempt = false` |
| `preempt_delay` | `preempt_delay` |
| `track_script` | Native health checks or explicit command check |
| `notify_*` | Event subscribers/control API |

### Phase 3: Migration utility

Provide:

```text
highland import-keepalived \
  --input /etc/keepalived/keepalived.conf \
  --output /etc/highland/config.toml
```

The importer must:

- Warn about unsupported directives.
- Preserve comments where possible.
- Never silently drop safety-relevant behavior.
- Produce a migration report.
- Validate the generated file.

---

## 25. Documentation Requirements

The project must include:

### User documentation

- Quick start
- Two-node setup
- IPv4 setup
- IPv6 setup
- Unicast setup
- Multicast setup
- Health checks
- Graceful shutdown
- Configuration reload
- Metrics
- Troubleshooting
- Keepalived migration

### Operator documentation

- Network requirements
- Firewall requirements
- Capabilities
- Split-brain behavior
- Packet capture diagnosis
- Failure recovery
- VIP conflict diagnosis
- Cloud deployment caveats
- Upgrade strategy
- Rollback strategy

### Developer documentation

- Architecture
- State machine design
- Protocol implementation
- Linux networking layer
- Testing approach
- Threat model
- Release process
- Compatibility policy

---

## 26. Versioning and Stability

Use Semantic Versioning for public crates.

### Pre-1.0

Before 1.0:

- Public APIs may change.
- Configuration format changes require migration notes.
- Protocol behavior must remain standards-compliant.
- The daemon should warn about unstable features.

### 1.0 requirements

Do not declare 1.0 until:

- IPv4 and IPv6 VRRPv3 are interoperable.
- Multicast and unicast modes work.
- VIP ownership is reliable.
- Network namespace integration tests are stable.
- Fuzzing is established.
- Configuration reload is safe.
- Upgrade and rollback procedures are documented.
- At least one real-world deployment has run for an extended period.
- Failure behavior is well understood.

---

## 27. Release Milestones

## Milestone 0: Repository and architecture

Deliver:

- Workspace layout
- CI
- Formatting and linting
- Documentation skeleton
- Error conventions
- Test conventions
- Feature flags
- MSRV policy

Suggested checks:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
```

## Milestone 1: Pure state machine

Deliver:

- Roles
- Events
- Timers
- Priority logic
- Preemption logic
- Health aggregation
- Deterministic fake clock
- Extensive unit and property tests

No Linux networking yet.

## Milestone 2: VRRP packet implementation

Deliver:

- Advertisement encoding
- Advertisement decoding
- Checksum
- Validation
- IPv4 and IPv6 packet tests
- Fuzz targets
- Packet-capture fixtures

## Milestone 3: Single-instance Linux daemon

Deliver:

- One interface
- One VRID
- IPv4
- Multicast or unicast
- VIP add/remove
- Basic state transitions
- Structured logs

## Milestone 4: Two-node integration

Deliver:

- Network namespace harness
- Master failure tests
- VIP movement
- Gratuitous ARP
- Configuration validation
- Graceful shutdown

## Milestone 5: IPv6 and unicast maturity

Deliver:

- IPv6 VIPs
- IPv6 advertisements
- Neighbor Advertisements
- Unicast peer mode
- Multiple peers
- Link and route diagnostics

## Milestone 6: Native health checks

Deliver:

- TCP
- HTTP
- Unix socket
- Interface checks
- Check thresholds
- Weighted priority
- Explainable demotion

## Milestone 7: Operations interface

Deliver:

- Control socket
- Status API
- Metrics
- Reload
- Pause/resume
- Relinquish
- Event stream

## Milestone 8: Compatibility and hardening

Deliver:

- Keepalived interoperability
- Configuration importer
- Chaos tests
- Security review
- Resource limits
- Capability documentation
- Packaging

## Milestone 9: 1.0 candidate

Deliver:

- Stable configuration subset
- Stable crate APIs
- Upgrade documentation
- Release artifacts
- Long-running test deployments
- Incident and failure playbooks

---

## 28. Recommended Initial Scope

The first implementation should deliberately support only:

- Linux
- IPv4
- VRRPv3
- One interface per instance
- One or more VIPs
- Unicast peers
- Priority election
- Preemption on/off
- Graceful relinquish
- TCP and HTTP health checks
- TOML configuration
- JSON status output
- Prometheus metrics
- Network namespace tests

Do not begin with:

- IPVS
- BFD
- Cloud APIs
- Shell scripts
- Dynamic routing
- Distributed consensus
- Full Keepalived configuration compatibility
- Non-Linux targets

This scope is sufficiently useful while remaining implementable.

---

## 29. Example Library API

The public API should be designed around reusable components rather than daemon internals.

```rust
use highland_core::{
    InstanceConfig,
    InstanceStateMachine,
    Event,
    Role,
};

let config = InstanceConfig {
    name: "api".into(),
    vrid: 42,
    priority: 150,
    ..Default::default()
};

let mut machine = InstanceStateMachine::new(config);

let actions = machine.handle(Event::Startup);

assert!(matches!(
    machine.role(),
    Role::Backup | Role::Init
));

for action in actions {
    println!("{action:?}");
}
```

Protocol usage:

```rust
use highland_vrrp::{
    Advertisement,
    decode_advertisement,
    encode_advertisement,
};

let advertisement = Advertisement {
    vrid: 42,
    priority: 150,
    ..Default::default()
};

let bytes = encode_advertisement(&advertisement, IpFamily::V4)?;
let decoded = decode_advertisement(&bytes, IpFamily::V4)?;

assert_eq!(decoded.vrid, 42);
```

Health-check usage:

```rust
use highland_checks::{Check, TcpCheck};

let check = TcpCheck::new("database", "127.0.0.1:5432")
    .timeout(Duration::from_millis(500))
    .failure_threshold(3);

let result = check.run().await?;
```

The exact API may change during planning, but the conceptual separation should remain.

---

## 30. Failure Scenarios the Design Must Handle

### Scenario 1: MASTER loses its application

Expected behavior:

1. HTTP health check fails.
2. Failure threshold is reached.
3. Effective priority changes or node becomes ineligible.
4. Highland emits a health transition.
5. Node relinquishes VIP according to policy.
6. BACKUP takes ownership.
7. Clients receive gratuitous ARP/ND updates.

### Scenario 2: MASTER loses the network interface

Expected behavior:

1. Link event is received.
2. Advertisement task stops.
3. VIP ownership is cleaned up if possible.
4. Backup detects advertisement timeout.
5. Backup becomes MASTER.

### Scenario 3: Higher-priority node returns

With preemption enabled:

1. Returning node completes startup delay.
2. Health checks pass.
3. Preemption delay begins.
4. Returning node advertises its priority.
5. Current MASTER relinquishes.
6. Returning node claims VIPs.

With preemption disabled:

1. Returning node remains BACKUP.
2. Current MASTER remains owner.

### Scenario 4: Both nodes start simultaneously

Expected behavior:

- Election converges.
- Equal-priority behavior is deterministic.
- No node claims ownership indefinitely without advertisements.
- Logs explain the winner.

### Scenario 5: Network partition

Expected behavior:

- The project documents possible dual-MASTER behavior.
- Nodes report peer loss.
- No false claim of consensus is made.
- Operators can inspect role history and peer state.
- Optional fencing can be added later.

### Scenario 6: VIP addition fails

Expected behavior:

- Node does not report normal MASTER readiness.
- State enters a controlled failure path.
- Retry behavior is bounded.
- Error includes interface, address, and kernel error.
- Other instances remain unaffected.

---

## 31. Performance Requirements

The project is infrastructure software, not a high-throughput packet processor. Prioritize correctness and predictable behavior.

Initial targets:

- Advertisement handling should not require a dedicated thread per packet.
- Idle memory usage should remain modest.
- Health checks must have explicit concurrency limits.
- A node should support at least dozens of instances without pathological behavior.
- Invalid packet floods must not cause unbounded CPU or memory usage.
- Metrics must not create unbounded label cardinality.
- Configuration reload should not interrupt unrelated instances.

Benchmark:

- Packet encode/decode throughput
- State-machine event throughput
- Configuration parse time
- Health-check scheduler overhead
- Netlink operation latency
- Metrics overhead
- Large instance-count behavior

---

## 32. Development Principles

1. Keep the protocol implementation independent from Linux.
2. Keep the state machine independent from async runtime details.
3. Keep network side effects behind traits or explicit executors.
4. Make invalid states difficult to represent.
5. Prefer typed configuration over stringly typed behavior.
6. Never hide ownership changes.
7. Never make shell execution the default.
8. Make failure explanations first-class.
9. Treat tests and packet captures as part of the product.
10. Prefer a smaller correct feature over a broad unreliable feature.
11. Do not promise fencing that VRRP cannot provide.
12. Make operational behavior observable before adding advanced features.

---

## 33. Definition of Done for the First Public Release

The first public release is ready when:

- A two-node Linux deployment can fail over an IPv4 VIP reliably.
- It interoperates with at least one existing VRRP implementation.
- It supports unicast peer mode.
- Health checks can trigger controlled demotion.
- State transitions are visible in logs and metrics.
- Configuration errors are clear and actionable.
- Network namespace integration tests pass consistently.
- Fuzz targets exist for all untrusted parsers.
- The daemon survives malformed packets and repeated network errors.
- Graceful shutdown removes ownership correctly.
- The project documents split-brain limitations.
- A user can install and operate it without reading the source code.
- A failed configuration reload leaves the previous configuration active.
- No core feature requires executing an arbitrary shell command.

---

## 34. Final Project Definition

Highland should be described as:

> Highland is a Rust-native Linux high-availability networking library and daemon that implements VRRP-based virtual IP failover with typed configuration, native health checks, deterministic state transitions, and first-class observability.

The project should not market itself as a complete Keepalived clone at the beginning. Its strongest identity is:

> **A safe, explainable, testable failover engine for Linux.**

That leaves room to support Keepalived migration while allowing Highland’s design to improve substantially on the traditional shell-script-driven model.
