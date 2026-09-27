// Rust guideline compliant 2026-09-27

//! Semantic validation of a configuration document.
//!
//! Each rule in this module corresponds to a `V-nn` requirement in
//! `SPEC.md` §10.4. Validation collects every violation rather than stopping at
//! the first, because an operator fixing a configuration should see all of its
//! problems at once.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::time::Duration;

use crate::duration::DurationSpec;
use crate::error::{ConfigViolation, ConfigViolations, Violation};
use crate::model::{
    CheckConfig, Config, FailurePolicy, Family, InstanceConfig, InstanceLimits, NetworkMode,
    SUPPORTED_SCHEMA_VERSION,
};

/// The lowest advertisement interval accepted by `V-04`.
const MIN_ADVERTISEMENT_INTERVAL: DurationSpec = DurationSpec(Duration::from_millis(10));
/// The highest advertisement interval accepted by `V-04`.
pub const MAX_ADVERTISEMENT_INTERVAL: DurationSpec = DurationSpec(Duration::from_millis(2550));
/// The largest number of instances accepted by `V-27`.
pub const MAX_INSTANCES: usize = 256;
/// The largest total electoral weight accepted by `V-20`.
pub const MAX_TOTAL_WEIGHT: u16 = 255;

/// The environmental facts validation needs but cannot derive from the
/// document.
#[derive(Debug, Clone)]
pub struct ValidationContext<'a> {
    /// Whether the binary was built with command checks available (`V-21`).
    pub command_checks_enabled: bool,
    /// Addresses configured on this node, used to reject self-peering (`V-08`).
    pub local_addresses: &'a [IpAddr],
    /// Interface existence, used to check binding (`V-22`).
    pub interfaces: &'a dyn InterfaceProbe,
    /// The per-instance hard limits (`V-25`).
    pub limits: InstanceLimits,
}

impl Default for ValidationContext<'_> {
    fn default() -> Self {
        Self {
            command_checks_enabled: false,
            local_addresses: &[],
            interfaces: &ANY_INTERFACE,
            limits: InstanceLimits::default(),
        }
    }
}

impl ValidationContext<'_> {
    /// Returns a context that assumes every interface exists, for tests and for
    /// `--check-config` on a foreign host.
    #[must_use]
    pub fn permissive() -> Self {
        Self::default()
    }
}

/// Reports whether an interface exists.
pub trait InterfaceProbe: std::fmt::Debug {
    /// Returns `true` when the named interface exists.
    fn interface_exists(&self, name: &str) -> bool;
}

/// An [`InterfaceProbe`] that reports every interface as present.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnyInterface;

impl InterfaceProbe for AnyInterface {
    fn interface_exists(&self, _name: &str) -> bool {
        true
    }
}

static ANY_INTERFACE: AnyInterface = AnyInterface;

/// An [`InterfaceProbe`] backed by a known set of interface names.
#[derive(Debug, Clone)]
pub struct KnownInterfaces<'a> {
    /// The names that exist.
    pub names: &'a BTreeSet<String>,
}

impl InterfaceProbe for KnownInterfaces<'_> {
    fn interface_exists(&self, name: &str) -> bool {
        self.names.contains(name)
    }
}

/// Validates a configuration, returning every violation found.
///
/// # Errors
///
/// Returns [`ConfigError::Invalid`](crate::ConfigError::Invalid) when at least
/// one rule is violated.
///
/// # Examples
///
/// ```
/// use highland_config::{ValidationContext, parse, validate};
///
/// let text = r#"
/// schema_version = 1
/// [node]
/// name = "node-a"
/// [[instance]]
/// name = "api"
/// interface = "eth0"
/// vrid = 0
/// [[instance.vip]]
/// address = "192.0.2.10/24"
/// [instance.network]
/// mode = "unicast"
/// peers = ["192.0.2.11"]
/// "#;
///
/// let config = parse(text).expect("the document parses");
/// let violations = validate(&config, &ValidationContext::permissive()).unwrap_err();
/// assert!(violations.iter().any(|violation| violation.rule == "V-01"));
/// ```
pub fn validate(config: &Config, context: &ValidationContext<'_>) -> Result<(), ConfigViolations> {
    let mut violations: Vec<Violation> = Vec::new();

    if !config.schema_version.is_supported() {
        violations.push((
            "V-28",
            format!(
                "schema_version {} is not supported; this release accepts {}",
                config.schema_version.0, SUPPORTED_SCHEMA_VERSION
            ),
        ));
    }
    if config.node.name.trim().is_empty() {
        violations.push(("V-32", "node.name must not be empty".to_owned()));
    }
    if config.instances.is_empty() {
        violations.push((
            "V-31",
            "at least one [[instance]] must be configured".to_owned(),
        ));
    }
    if config.instances.len() > MAX_INSTANCES {
        violations.push((
            "V-27",
            format!(
                "{} instances are configured, above the limit of {MAX_INSTANCES}",
                config.instances.len()
            ),
        ));
    }
    validate_metrics(config, &mut violations);

    let mut identities: BTreeSet<(&str, u8)> = BTreeSet::new();
    let mut vip_owners: BTreeMap<IpAddr, String> = BTreeMap::new();
    let mut seen_names: BTreeSet<&str> = BTreeSet::new();

    for instance in &config.instances {
        let key = format!("instance.{}", instance.name);
        if !seen_names.insert(instance.name.as_str()) {
            violations.push((
                "V-05",
                format!(
                    "{key}: instance name {:?} is used more than once",
                    instance.name
                ),
            ));
        }
        if !identities.insert((instance.interface.as_str(), instance.vrid)) {
            violations.push((
                "V-06",
                format!(
                    "{key}: interface {:?} already runs VRID {}; (interface, vrid) must be unique",
                    instance.interface, instance.vrid
                ),
            ));
        }

        validate_instance(instance, context, &key, &mut vip_owners, &mut violations);
    }

    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations
            .into_iter()
            .map(|(rule, message)| ConfigViolation { rule, message })
            .collect())
    }
}

fn validate_metrics(config: &Config, violations: &mut Vec<Violation>) {
    match (&config.metrics.enabled, &config.metrics.listen) {
        (true, None) => {
            violations.push((
                "V-30",
                "metrics.enabled is true but metrics.listen is not set".to_owned(),
            ));
        }
        (false, Some(_)) => {
            violations.push((
                "V-30",
                "metrics.listen is set but metrics.enabled is false".to_owned(),
            ));
        }
        _ => {}
    }
}

fn validate_instance(
    instance: &InstanceConfig,
    context: &ValidationContext<'_>,
    key: &str,
    vip_owners: &mut BTreeMap<IpAddr, String>,
    violations: &mut Vec<Violation>,
) {
    if instance.vrid == 0 {
        violations.push((
            "V-01",
            format!("{key}.vrid: 0 is not a valid VRID; use 1..=255"),
        ));
    }
    if instance.priority == 0 {
        violations.push((
            "V-02",
            format!("{key}.priority: 0 is reserved for relinquishment; use 1..=255"),
        ));
    }
    let interval = instance.advertisement_interval;
    if !(MIN_ADVERTISEMENT_INTERVAL..=MAX_ADVERTISEMENT_INTERVAL).contains(&interval) {
        violations.push((
            "V-04",
            format!("{key}.advertisement_interval: {interval:?} is outside 10ms..=2550ms"),
        ));
    }
    if !instance.preempt && !instance.preempt_delay.is_zero() {
        violations.push((
            "V-10",
            format!(
                "{key}.preempt_delay: set to {} while preempt is false",
                instance.preempt_delay
            ),
        ));
    }
    if !instance.defer_interface_binding
        && !context.interfaces.interface_exists(&instance.interface)
    {
        violations.push((
            "V-22",
            format!(
                "{key}.interface: {:?} does not exist; set defer_interface_binding to accept it",
                instance.interface
            ),
        ));
    }

    validate_vips(instance, key, vip_owners, violations);
    validate_network(instance, context, key, violations);
    validate_health(instance, key, violations);
    validate_limits(instance, key, context, violations);
}

fn validate_vips(
    instance: &InstanceConfig,
    key: &str,
    vip_owners: &mut BTreeMap<IpAddr, String>,
    violations: &mut Vec<Violation>,
) {
    if instance.vips.is_empty() {
        violations.push((
            "V-11",
            format!("{key}: at least one [[instance.vip]] must be configured"),
        ));
        return;
    }

    let mut prefixes: BTreeMap<IpAddr, u8> = BTreeMap::new();
    for (index, vip) in instance.vips.iter().enumerate() {
        let location = format!("{key}.vip[{index}].address");
        let Ok((address, prefix_len)) = parse_cidr(&vip.address) else {
            violations.push((
                "V-13",
                format!(
                    "{location}: {:?} is not a valid address in CIDR notation",
                    vip.address
                ),
            ));
            continue;
        };
        if prefix_len == 0 {
            violations.push((
                "V-13",
                format!("{location}: a prefix length of 0 would install a default route"),
            ));
        }
        if let Some(previous) = prefixes.insert(address, prefix_len) {
            let text = vip.address.clone();
            let message = if previous == prefix_len {
                format!("{location}: {text} is configured twice in the same instance")
            } else {
                format!(
                    "{key}: {address} is configured with prefix lengths {previous} and {prefix_len}"
                )
            };
            violations.push(("V-12", message));
        } else if let Some(owner) = vip_owners.get(&address) {
            violations.push((
                "V-12",
                format!("{location}: {address} is already managed by instance {owner:?}"),
            ));
        } else {
            vip_owners.insert(address, instance.name.clone());
        }
    }

    if instance.vip_families().len() > 1 {
        let families: Vec<&str> = instance
            .vip_families()
            .iter()
            .map(|family| match family {
                Family::V4 => "IPv4",
                Family::V6 => "IPv6",
            })
            .collect();
        violations.push((
            "V-03",
            format!(
                "{key}: an instance may not mix {} in this release; split it into one instance per family",
                families.join(" and ")
            ),
        ));
    }
}

fn validate_network(
    instance: &InstanceConfig,
    context: &ValidationContext<'_>,
    key: &str,
    violations: &mut Vec<Violation>,
) {
    for (index, peer) in instance.network.peers.iter().enumerate() {
        let location = format!("{key}.network.peers[{index}]");
        if peer.is_multicast() {
            violations.push((
                "V-09",
                format!("{location}: {peer} is a multicast address and cannot be a unicast peer"),
            ));
        }
        if context.local_addresses.contains(peer) {
            violations.push((
                "V-08",
                format!("{location}: {peer} is configured on this node and cannot be its own peer"),
            ));
        }
    }

    if instance.network.mode == NetworkMode::Multicast && instance.network.multicast.ttl != 255 {
        violations.push((
            "V-24",
            format!(
                "{key}.network.multicast.ttl: {} is invalid; VRRP requires 255",
                instance.network.multicast.ttl
            ),
        ));
    }

    if instance.network.mode == NetworkMode::Unicast {
        for family in instance.vip_families() {
            if instance.network.peers_for(family).is_empty() {
                let name = match family {
                    Family::V4 => "IPv4",
                    Family::V6 => "IPv6",
                };
                violations.push((
                    "V-07",
                    format!("{key}.network.peers: the instance has {name} VIPs but no {name} peer"),
                ));
            }
        }
    }
}

fn validate_health(instance: &InstanceConfig, key: &str, violations: &mut Vec<Violation>) {
    let health = &instance.health;
    let minimum_is_default = health.minimum_effective_priority == 1;

    match health.failure_policy {
        FailurePolicy::FailClosed => {
            if !minimum_is_default {
                violations.push((
                    "V-16",
                    format!(
                        "{key}.health.minimum_effective_priority: not meaningful under fail_closed, where a failing check makes the instance ineligible"
                    ),
                ));
            }
        }
        FailurePolicy::Manual => {
            if !minimum_is_default {
                violations.push((
                    "V-17",
                    format!(
                        "{key}.health.minimum_effective_priority: not meaningful under manual, where health never changes election"
                    ),
                ));
            }
            if health.all_checks_required {
                violations.push((
                    "V-17",
                    format!("{key}.health.all_checks_required: not meaningful under manual"),
                ));
            }
            if instance.electoral_weight() > 0 {
                violations.push((
                    "V-19",
                    format!(
                        "{key}: checks with a non-zero weight cannot affect election under the manual policy"
                    ),
                ));
            }
        }
        FailurePolicy::Weighted => {
            if health.all_checks_required {
                violations.push((
                    "V-18",
                    format!(
                        "{key}.health.all_checks_required: only valid under fail_closed, not weighted"
                    ),
                ));
            }
            if !health.send_zero_priority_advert {
                violations.push((
                    "V-18",
                    format!(
                        "{key}.health.send_zero_priority_advert: only valid under fail_closed, not weighted"
                    ),
                ));
            }
            if instance.electoral_weight() > MAX_TOTAL_WEIGHT {
                violations.push((
                    "V-20",
                    format!(
                        "{key}: the total check weight {} exceeds {MAX_TOTAL_WEIGHT}",
                        instance.electoral_weight()
                    ),
                ));
            }
        }
    }
}

fn validate_limits(
    instance: &InstanceConfig,
    key: &str,
    context: &ValidationContext<'_>,
    violations: &mut Vec<Violation>,
) {
    let limits = &context.limits;
    if instance.vips.len() > limits.max_vips {
        violations.push((
            "V-25",
            format!(
                "{key}: {} VIPs exceed the limit of {}",
                instance.vips.len(),
                limits.max_vips
            ),
        ));
    }
    if instance.network.peers.len() > limits.max_peers {
        violations.push((
            "V-25",
            format!(
                "{key}: {} peers exceed the limit of {}",
                instance.network.peers.len(),
                limits.max_peers
            ),
        ));
    }
    if instance.checks.len() > limits.max_checks {
        violations.push((
            "V-25",
            format!(
                "{key}: {} checks exceed the limit of {}",
                instance.checks.len(),
                limits.max_checks
            ),
        ));
    }

    let mut names: BTreeSet<&str> = BTreeSet::new();
    for (index, check) in instance.checks.iter().enumerate() {
        let location = format!("{key}.check[{index}]");
        if !names.insert(check.name.as_str()) {
            violations.push((
                "V-29",
                format!(
                    "{location}: check name {:?} is used more than once",
                    check.name
                ),
            ));
        }
        validate_check(check, &location, context, violations);
    }
}

fn validate_check(
    check: &CheckConfig,
    location: &str,
    context: &ValidationContext<'_>,
    violations: &mut Vec<Violation>,
) {
    if check.timeout.is_zero() {
        violations.push((
            "V-14",
            format!("{location}.timeout: a timeout of 0 is not allowed"),
        ));
    }
    if check.interval.is_zero() {
        violations.push((
            "V-14",
            format!("{location}.interval: an interval of 0 is not allowed"),
        ));
    }
    if !check.timeout.is_zero()
        && !check.interval.is_zero()
        && check.timeout.as_duration() > check.interval.as_duration()
    {
        violations.push((
            "V-14",
            format!(
                "{location}: timeout {} exceeds interval {}",
                check.timeout, check.interval
            ),
        ));
    }
    if check.failure_threshold == 0 {
        violations.push((
            "V-15",
            format!("{location}.failure_threshold: thresholds start at 1"),
        ));
    }
    if check.success_threshold == 0 {
        violations.push((
            "V-15",
            format!("{location}.success_threshold: thresholds start at 1"),
        ));
    }

    let Some(required) = check.required_keys() else {
        violations.push((
            "V-23",
            format!(
                "{location}.type: {:?} is not a known check type",
                check.check_type
            ),
        ));
        return;
    };

    for key in required {
        if !check_has_value(check, key) {
            violations.push((
                "V-23",
                format!(
                    "{location}: a {:?} check requires the {key:?} key",
                    check.check_type
                ),
            ));
        }
    }

    if check.check_type == "command" {
        validate_command_check(check, location, context, violations);
    }
}

fn validate_command_check(
    check: &CheckConfig,
    location: &str,
    context: &ValidationContext<'_>,
    violations: &mut Vec<Violation>,
) {
    if !context.command_checks_enabled {
        violations.push((
            "V-21",
            format!(
                "{location}: command checks require a binary built with the `command-checks` feature"
            ),
        ));
        return;
    }
    let Some(allow_paths) = check.allow_paths.as_ref() else {
        violations.push((
            "V-21",
            format!("{location}.allow_paths: command checks require an explicit allow-list"),
        ));
        return;
    };
    for (index, path) in allow_paths.iter().enumerate() {
        if !path.starts_with('/') {
            violations.push((
                "V-21",
                format!("{location}.allow_paths[{index}]: {path:?} must be an absolute path"),
            ));
        }
    }
    if let Some(command) = check.command.as_ref().and_then(|parts| parts.first()) {
        if !allow_paths.contains(command) {
            violations.push((
                "V-21",
                format!("{location}.command: {command:?} is not in the allow-list"),
            ));
        }
    }
}

fn check_has_value(check: &CheckConfig, key: &str) -> bool {
    match key {
        "address" => check.address.is_some(),
        "url" => check.url.is_some(),
        "expected_status" => check
            .expected_status
            .as_ref()
            .is_some_and(|codes| !codes.is_empty()),
        "record" => check.record.is_some(),
        "path" => check.path.is_some(),
        "interface" => check.interface.is_some(),
        "command" => check
            .command
            .as_ref()
            .is_some_and(|parts| !parts.is_empty()),
        "members" => true,
        _ => false,
    }
}

/// Parses `address/prefix` notation, returning the address and prefix length.
fn parse_cidr(text: &str) -> Result<(IpAddr, u8), ()> {
    let (address, prefix) = text.split_once('/').ok_or(())?;
    let address: IpAddr = address.parse().map_err(|_| ())?;
    let prefix: u8 = prefix.parse().map_err(|_| ())?;
    let max = if matches!(address, IpAddr::V4(_)) {
        32
    } else {
        128
    };
    if prefix > max {
        return Err(());
    }
    Ok((address, prefix))
}
