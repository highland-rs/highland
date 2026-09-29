// Rust guideline compliant 2026-09-27

//! The typed configuration model.
//!
//! Every field maps one-to-one onto a configuration key. Unknown keys are
//! rejected rather than ignored, so a typo is a startup failure instead of a
//! silently ignored setting (SPEC.md, `I-08`).

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::duration::DurationSpec;

/// The only schema version this release accepts (`V-28`).
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// Why a check type this release names cannot be run (`V-23`).
///
/// Every type below is a *known* type -- `V-23` accepts the spelling and knows
/// which keys it needs -- and only four of them are implemented. The rest are
/// declared so that a configuration using one is refused with a specific reason
/// rather than an "unknown type", and so that the refusal happens at
/// `check-config` instead of at run time.
///
/// The table lives here, beside `CHECK_KEY_TYPES`, and the daemon reads it
/// through [`unimplemented_check_reason`]. That is deliberate: the runtime and
/// the validator must not be able to disagree about which types work, because
/// the failure mode of disagreeing is a node that starts, logs that a check
/// cannot be built, and then takes its VIP without the check that was supposed
/// to gate it.
pub const UNIMPLEMENTED_CHECK_TYPES: &[(&str, &str)] = &[
    (
        "https",
        "an https check needs TLS, which this build does not implement; use a tcp check on \
         the same port rather than a check that cannot validate a certificate",
    ),
    ("dns", "a dns check is not implemented in this release"),
    (
        "process",
        "a process check is not implemented; existence is a weak signal that says nothing \
         about readiness",
    ),
    ("file", "a file check is not implemented in this release"),
    (
        "composite",
        "a composite check is not implemented in this release",
    ),
    (
        "command",
        "command checks require the command-checks feature and an explicit allow-list",
    ),
];

/// Returns why `kind` cannot be run by this build, or `None` if it can.
///
/// The single answer to "can this build run this check", shared by the validator
/// and the daemon so that a configuration accepted by `check-config` is a
/// configuration the daemon can actually run.
#[must_use]
pub fn unimplemented_check_reason(kind: &str) -> Option<&'static str> {
    UNIMPLEMENTED_CHECK_TYPES
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, reason)| *reason)
}

/// The configuration keys that each check type requires (`V-23`).
pub const CHECK_KEY_TYPES: &[(&str, &[&str])] = &[
    ("tcp", &["address"]),
    ("http", &["url", "expected_status"]),
    ("https", &["url", "expected_status"]),
    ("dns", &["record"]),
    ("unix", &["path"]),
    ("process", &["process"]),
    ("interface", &["interface"]),
    ("file", &["path"]),
    ("composite", &[]),
    ("command", &["command"]),
];

/// The schema version declared by a configuration document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersion(pub u32);

impl SchemaVersion {
    /// Returns `true` when this release can load the version.
    #[must_use]
    pub fn is_supported(self) -> bool {
        self.0 == SUPPORTED_SCHEMA_VERSION
    }
}

/// How peers are addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// Explicit peer list. The only mode in the initial public release.
    #[default]
    Unicast,
    /// A multicast group per address family. Available from 1.0.
    Multicast,
}

/// The node-level configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    /// The node name, used in every event and log line.
    pub name: String,
}

/// The logging configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// The maximum level emitted.
    pub level: String,
    /// The rendering style.
    pub format: String,
    /// Configuration key paths whose values are redacted (`S-01`).
    #[serde(default)]
    pub redact: Vec<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: "text".to_owned(),
            redact: Vec::new(),
        }
    }
}

/// The metrics configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MetricsConfig {
    /// Whether the Prometheus endpoint is served.
    pub enabled: bool,
    /// The listen address, required when enabled (`V-30`).
    pub listen: Option<String>,
}

/// The control-socket configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ControlConfig {
    /// The socket path.
    pub socket: String,
    /// The group allowed to use the socket.
    pub group: Option<String>,
    /// Whether peer credentials are verified.
    ///
    /// The default comes from [`ControlConfig::default`], which is `true`. A
    /// field-level `#[serde(default)]` here would mean `bool::default()`, which
    /// is `false`, and would silently turn a documented secure default into an
    /// insecure one.
    pub verify_peer_credentials: bool,
}

impl Default for ControlConfig {
    fn default() -> Self {
        Self {
            socket: "/run/highland/control.sock".to_owned(),
            group: None,
            verify_peer_credentials: true,
        }
    }
}

/// The multicast settings for one address family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MulticastConfig {
    /// The group address, or `None` for the RFC's default *for the instance's
    /// family*.
    ///
    /// An unset group is per family (`224.0.0.18` for IPv4, `ff02::12` for IPv6),
    /// which is why this is not a plain `IpAddr` with a default value: a single
    /// default would be the wrong family for half the instances, and a
    /// configuration that silently joined the wrong group would look healthy
    /// while hearing nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<IpAddr>,
    /// The TTL. Must be 255 (`V-24`).
    #[serde(default = "default_multicast_ttl")]
    pub ttl: u8,
}

impl MulticastConfig {
    /// Returns the group an instance of `family` speaks for.
    ///
    /// A configured group of that family is used; anything else is the RFC's
    /// default for the family, which is what validation refuses a mismatched
    /// group for.
    ///
    /// # Panics
    ///
    /// Never. The two defaults are literals, and the address they are parsed
    /// into cannot fail to parse.
    #[must_use]
    pub fn group_for(&self, family: Family) -> IpAddr {
        match (family, self.group) {
            (Family::V4, Some(IpAddr::V4(address))) => IpAddr::V4(address),
            (Family::V6, Some(IpAddr::V6(address))) => IpAddr::V6(address),
            (Family::V4, _) => "224.0.0.18"
                .parse()
                .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            (Family::V6, _) => "ff02::12"
                .parse()
                .unwrap_or(IpAddr::V6(Ipv6Addr::UNSPECIFIED)),
        }
    }
}

impl Default for MulticastConfig {
    fn default() -> Self {
        Self {
            group: None,
            ttl: default_multicast_ttl(),
        }
    }
}

fn default_multicast_ttl() -> u8 {
    255
}

/// The peer transport configuration for one instance.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    /// The peer addressing mode.
    pub mode: NetworkMode,
    /// The unicast peer list. The family is inferred from each entry.
    #[serde(default)]
    pub peers: Vec<IpAddr>,
    /// The multicast settings, used when `mode` is `multicast`.
    #[serde(default)]
    pub multicast: MulticastConfig,
    /// Accept advertisements whose TTL or hop limit is not 255.
    ///
    /// Off by default, and off means the check is enforced: RFC 5798 §5.1.1.3 and
    /// §5.1.2.3 both say a receiver MUST discard such a packet, and the reason is
    /// that a value of 255 is the only one a packet can have if it did not cross a
    /// router. Turning it on gives up that guarantee — an advertisement that
    /// arrived from off this link would be believed — in exchange for hearing a
    /// peer that gets it wrong.
    ///
    /// This exists for Keepalived 2.3.3, which sends IPv6 *unicast*
    /// advertisements with a hop limit of 64 and has no setting to change it:
    /// `hop_limit` is rejected in both `vrrp_instance` and `global_defs`. Its IPv6
    /// *multicast* advertisements carry 255 and need nothing.
    ///
    /// The daemon logs a warning and counts a metric for every packet accepted
    /// only because of this, so an operator is never guessing whether they are in
    /// the degraded mode.
    #[serde(default)]
    pub allow_unconforming_hop_limit: bool,
}

impl NetworkConfig {
    /// Returns the peers that belong to `family`.
    #[must_use]
    pub fn peers_for(&self, family: Family) -> Vec<IpAddr> {
        self.peers
            .iter()
            .copied()
            .filter(|peer| family.matches(peer))
            .collect()
    }
}

/// The address family of a VIP or peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    /// IPv4.
    V4,
    /// IPv6.
    V6,
}

impl Family {
    /// Returns `true` when `address` belongs to this family.
    #[must_use]
    pub fn matches(self, address: &IpAddr) -> bool {
        matches!(
            (self, address),
            (Family::V4, IpAddr::V4(_)) | (Family::V6, IpAddr::V6(_))
        )
    }

    /// Returns the family of `address`.
    #[must_use]
    pub fn of(address: &IpAddr) -> Self {
        match address {
            IpAddr::V4(_) => Family::V4,
            IpAddr::V6(_) => Family::V6,
        }
    }
}

/// A virtual IP address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VipConfig {
    /// The address in CIDR notation.
    pub address: String,
}

impl VipConfig {
    /// Returns the address part, without the prefix.
    #[must_use]
    pub fn ip_address(&self) -> Option<IpAddr> {
        self.address
            .split_once('/')
            .map_or(self.address.as_str(), |(address, _)| address)
            .parse()
            .ok()
    }

    /// Returns the family this VIP belongs to, when the text is well formed.
    #[must_use]
    pub fn family(&self) -> Option<Family> {
        self.ip_address().map(|address| Family::of(&address))
    }
}

/// The health policy applied when a check fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailurePolicy {
    /// Failing checks make the instance ineligible immediately.
    FailClosed,
    /// Failing checks reduce the effective priority by their weight.
    #[default]
    Weighted,
    /// Health never changes election state.
    Manual,
}

/// The instance health configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HealthConfig {
    /// The policy applied when a check fails.
    #[serde(default)]
    pub failure_policy: FailurePolicy,
    /// The floor applied to the effective priority under `weighted`.
    #[serde(default = "default_minimum_effective_priority")]
    pub minimum_effective_priority: u8,
    /// Whether any failing check blocks ownership under `fail_closed`.
    #[serde(default)]
    pub all_checks_required: bool,
    /// Whether a relinquishing `MASTER` announces priority 0.
    #[serde(default = "default_true")]
    pub send_zero_priority_advert: bool,
    /// Extra debounce applied before health changes affect election.
    #[serde(default)]
    pub debounce: DurationSpec,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            failure_policy: FailurePolicy::Weighted,
            minimum_effective_priority: 1,
            all_checks_required: false,
            send_zero_priority_advert: true,
            debounce: DurationSpec::default(),
        }
    }
}

/// One health check.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckConfig {
    /// The check name, unique within its instance (`V-29`).
    pub name: String,
    /// The probe type.
    #[serde(rename = "type")]
    pub check_type: String,
    /// The priority penalty while failing. Zero is observational.
    #[serde(default)]
    pub weight: u16,
    /// How often the probe runs.
    pub interval: DurationSpec,
    /// How long a probe may take.
    pub timeout: DurationSpec,
    /// Consecutive failures required to enter the failing state.
    pub failure_threshold: u32,
    /// Consecutive successes required to recover.
    pub success_threshold: u32,
    /// Failures are ignored for this long after the check starts.
    #[serde(default)]
    pub initial_grace_period: DurationSpec,
    /// Delay between retries inside one interval. Zero means one attempt.
    #[serde(default)]
    pub retry_interval: DurationSpec,
    /// The target address for `tcp`.
    pub address: Option<String>,
    /// The target URL for `http` and `https`.
    pub url: Option<String>,
    /// The accepted status codes for `http` and `https`.
    pub expected_status: Option<Vec<u16>>,
    /// The query name for `dns` checks.
    pub record: Option<String>,
    /// The socket path for `unix`.
    pub path: Option<String>,
    /// The interface name for `interface` checks.
    pub interface: Option<String>,
    /// The process name for `process` checks.
    pub process: Option<String>,
    /// The command and arguments for `command` checks.
    pub command: Option<Vec<String>>,
    /// The allowed executables for `command` checks (`R-06`).
    pub allow_paths: Option<Vec<String>>,
    /// Whether TLS verification is disabled for `https`.
    #[serde(default)]
    pub insecure: bool,
}

impl CheckConfig {
    /// Returns the keys this check type requires (`V-23`).
    #[must_use]
    pub fn required_keys(&self) -> Option<&'static [&'static str]> {
        CHECK_KEY_TYPES
            .iter()
            .find(|(kind, _)| *kind == self.check_type)
            .map(|(_, required)| *required)
    }
}

/// One VRRP instance.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceConfig {
    /// The instance name, unique in the file (`V-05`).
    pub name: String,
    /// The interface the instance is bound to.
    pub interface: String,
    /// Whether interface existence is checked at load time.
    #[serde(default)]
    pub defer_interface_binding: bool,
    /// The Virtual Router Identifier.
    pub vrid: u8,
    /// The configured priority.
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// The advertisement interval.
    #[serde(default = "default_advertisement_interval")]
    pub advertisement_interval: DurationSpec,
    /// Whether a higher-priority `BACKUP` may preempt.
    #[serde(default = "default_true")]
    pub preempt: bool,
    /// The delay before preemption. Forbidden when `preempt` is false (`V-10`).
    #[serde(default)]
    pub preempt_delay: DurationSpec,
    /// The delay between startup and entering election.
    #[serde(default)]
    pub startup_delay: DurationSpec,
    /// The virtual addresses.
    #[serde(default, rename = "vip")]
    pub vips: Vec<VipConfig>,
    /// The peer transport settings.
    #[serde(default)]
    pub network: NetworkConfig,
    /// The health policy.
    #[serde(default)]
    pub health: HealthConfig,
    /// The checks.
    #[serde(default, rename = "check")]
    pub checks: Vec<CheckConfig>,
}

impl InstanceConfig {
    /// Returns the configured VIPs that could be parsed as addresses.
    #[must_use]
    pub fn vip_addresses(&self) -> Vec<&str> {
        self.vips.iter().map(|vip| vip.address.as_str()).collect()
    }

    /// Returns the distinct families this instance has VIPs in.
    #[must_use]
    pub fn vip_families(&self) -> BTreeSet<Family> {
        self.vips.iter().filter_map(VipConfig::family).collect()
    }

    /// Returns the sum of the weights of the checks that can affect election.
    #[must_use]
    pub fn electoral_weight(&self) -> u16 {
        self.checks.iter().map(|check| check.weight).sum()
    }
}

fn default_true() -> bool {
    true
}

fn default_priority() -> u8 {
    100
}

fn default_advertisement_interval() -> DurationSpec {
    DurationSpec(Duration::from_secs(1))
}

fn default_minimum_effective_priority() -> u8 {
    1
}

/// The per-instance hard limits (`L-02`, `L-03`, `L-25`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceLimits {
    /// The largest number of VIPs one instance may configure (`L-02`).
    pub max_vips: usize,
    /// The largest number of unicast peers one instance may configure (`L-03`).
    pub max_peers: usize,
    /// The largest number of checks one instance may configure (`L-05`).
    pub max_checks: usize,
}

impl Default for InstanceLimits {
    fn default() -> Self {
        Self {
            max_vips: 255,
            max_peers: 255,
            max_checks: 64,
        }
    }
}

/// A complete configuration document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The schema version. Required; absence is a schema error (`V-28`).
    pub schema_version: SchemaVersion,
    /// The node identity.
    pub node: NodeConfig,
    /// The logging settings.
    #[serde(default)]
    pub logging: LoggingConfig,
    /// The metrics settings.
    #[serde(default)]
    pub metrics: MetricsConfig,
    /// The control-socket settings.
    #[serde(default)]
    pub control: ControlConfig,
    /// The instances, at least one.
    #[serde(default, rename = "instance")]
    pub instances: Vec<InstanceConfig>,
}
