// Rust guideline compliant 2026-09-27

//! Reload planning: what a candidate configuration would do to a running node.
//!
//! A reload is a transaction. Either every instance can absorb the change, or
// nothing is applied and the operator is told exactly which instance refused
//! and why (`I-09`, `R-46`). A partial reload that leaves one instance on old
//! settings is precisely the state this module exists to prevent.
//!
//! The planner is pure: it takes the running configuration and the candidate,
//! and returns a decision. It does not apply anything, so it can be tested
//! exhaustively without a kernel, a socket, or a running daemon.

use highland_config::{Config, InstanceConfig};

use crate::options::InstancePlan;

/// How a change to one instance would be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Nothing that affects the instance changed.
    None,
    /// The instance can absorb the change without restarting.
    Reloadable {
        /// What changed, in the words an operator would use.
        fields: Vec<String>,
    },
    /// The instance must be restarted, which a reload will not do.
    RestartRequired {
        /// What changed.
        fields: Vec<String>,
    },
    /// The instance is new, and will be started.
    Added,
    /// The instance is gone, and would have to be stopped.
    Removed,
}

impl Change {
    /// Returns `true` when a reload may proceed.
    #[must_use]
    pub fn is_applicable(&self) -> bool {
        matches!(
            self,
            Change::None | Change::Reloadable { .. } | Change::Added
        )
    }

    /// Returns the fields that changed, for a diagnostic.
    #[must_use]
    pub fn fields(&self) -> &[String] {
        match self {
            Change::Reloadable { fields } | Change::RestartRequired { fields } => fields,
            _ => &[],
        }
    }
}

/// What a reload would do to every instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The changes, keyed by instance name, in name order.
    pub changes: Vec<(String, Change)>,
}

impl Plan {
    /// Returns the instances that need a restart.
    #[must_use]
    pub fn restarts(&self) -> Vec<&(String, Change)> {
        self.changes
            .iter()
            .filter(|(_, change)| !change.is_applicable())
            .collect()
    }

    /// Returns `true` when the whole plan can be applied.
    #[must_use]
    pub fn is_applicable(&self) -> bool {
        self.restarts().is_empty()
    }

    /// Returns the instances that can be reconfigured in place.
    #[must_use]
    pub fn reloadable(&self) -> Vec<&str> {
        self.changes
            .iter()
            .filter_map(|(name, change)| {
                matches!(change, Change::Reloadable { .. }).then_some(name.as_str())
            })
            .collect()
    }

    /// Returns the instances that are new.
    #[must_use]
    pub fn added(&self) -> Vec<&str> {
        self.changes
            .iter()
            .filter_map(|(name, change)| matches!(change, Change::Added).then_some(name.as_str()))
            .collect()
    }

    /// Explains why the plan cannot be applied, one line per instance.
    #[must_use]
    pub fn refusals(&self) -> Vec<String> {
        self.restarts()
            .iter()
            .map(|(name, change)| {
                let fields = change.fields().join(", ");
                let fields = if fields.is_empty() {
                    "the instance was removed".to_owned()
                } else {
                    fields
                };
                format!("{name} would need a restart: {fields}")
            })
            .collect()
    }
}

/// Compares a running configuration with a candidate.
#[must_use]
pub fn plan(running: &Config, candidate: &Config) -> Plan {
    let mut changes = Vec::new();

    for instance in &running.instances {
        match candidate
            .instances
            .iter()
            .find(|other| other.name == instance.name)
        {
            None => changes.push((instance.name.clone(), Change::Removed)),
            Some(other) => changes.push((instance.name.clone(), classify(instance, other))),
        }
    }
    for instance in &candidate.instances {
        if !running
            .instances
            .iter()
            .any(|other| other.name == instance.name)
        {
            changes.push((instance.name.clone(), Change::Added));
        }
    }
    changes.sort_by(|left, right| left.0.cmp(&right.0));

    Plan { changes }
}

/// Classifies the change to one instance.
#[must_use]
pub fn classify(running: &InstanceConfig, candidate: &InstanceConfig) -> Change {
    // A change that cannot be absorbed: it is bound at startup, or it is
    // ownership, or it is the rate the running timer was armed from.
    let mut restart = Vec::new();
    if running.interface != candidate.interface {
        restart.push("interface".to_owned());
    }
    if running.vrid != candidate.vrid {
        restart.push("vrid".to_owned());
    }
    if running.network.mode != candidate.network.mode {
        restart.push("network.mode".to_owned());
    }
    // The rest of the network configuration is read once, when the socket is
    // bound: the peer list is the `PeerSet` the transport was built with, the
    // multicast group and TTL are socket options set at that moment, and
    // `allow_unconforming_hop_limit` is a value passed into the bind. None of
    // them can be changed underneath a running socket.
    //
    // They were not compared at all, so a change to any of them was classified
    // as `Change::None`: the reload reported itself applied, the generation went
    // up, the event history recorded a change, and the node kept using the
    // values it had started with. The hop-limit flag is the sharpest case -- it
    // is the one that relaxes TTL enforcement, so a file that says it is off and
    // a running socket that has it on is a security-relevant divergence that
    // nothing reports.
    if running.network.peers != candidate.network.peers {
        restart.push("network.peers".to_owned());
    }
    if running.network.multicast != candidate.network.multicast {
        restart.push("network.multicast".to_owned());
    }
    if running.network.allow_unconforming_hop_limit
        != candidate.network.allow_unconforming_hop_limit
    {
        restart.push("network.allow_unconforming_hop_limit".to_owned());
    }
    if running.vip_addresses() != candidate.vip_addresses() {
        restart.push("vip".to_owned());
    }

    let mut reloadable = Vec::new();
    if running.priority != candidate.priority {
        reloadable.push("priority".to_owned());
    }
    if running.preempt != candidate.preempt {
        reloadable.push("preempt".to_owned());
    }
    if running.preempt_delay != candidate.preempt_delay {
        reloadable.push("preempt_delay".to_owned());
    }
    if running.startup_delay != candidate.startup_delay {
        reloadable.push("startup_delay".to_owned());
    }
    if running.advertisement_interval != candidate.advertisement_interval {
        // The advertisement timer is already armed from the running value, so
        // changing it underneath a master would leave the timer firing at the
        // old rate.
        restart.push("advertisement_interval".to_owned());
    }
    if running.health != candidate.health {
        reloadable.push("health".to_owned());
    }
    // A change in the check list needs a restart for now, and saying so is the
    // honest classification: the running scheduler holds probes it built from the
    // old list, and a reload that reported "applied" while leaving those probes
    // in place would be a reload that did nothing.
    if running.checks != candidate.checks {
        restart.push("check".to_owned());
    }

    if !restart.is_empty() {
        restart.extend(reloadable);
        return Change::RestartRequired { fields: restart };
    }
    if reloadable.is_empty() {
        return Change::None;
    }
    Change::Reloadable { fields: reloadable }
}

/// Converts a configured instance into the plan the executor applies.
#[must_use]
pub fn plan_for(instance: &InstanceConfig) -> InstancePlan {
    InstancePlan::from_config(instance)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = r#"
schema_version = 1
[node]
name = "node-a"

[[instance]]
name = "api"
interface = "eth0"
vrid = 42
priority = 150
advertisement_interval = "1s"
startup_delay = "2s"

[instance.network]
mode = "unicast"
peers = ["192.0.2.11"]

[[instance.vip]]
address = "192.0.2.100/24"
"#;

    fn config(text: &str) -> Config {
        highland_config::parse(text).expect("the fixture parses")
    }

    #[test]
    fn an_unchanged_document_changes_nothing() {
        let plan = plan(&config(RUNNING), &config(RUNNING));

        assert!(plan.is_applicable());
        assert!(plan.reloadable().is_empty());
        assert_eq!(plan.changes, [("api".to_owned(), Change::None)]);
    }

    #[test]
    fn a_priority_change_is_reloadable() {
        let candidate = RUNNING.replace("priority = 150", "priority = 100");
        let plan = plan(&config(RUNNING), &config(&candidate));

        assert!(plan.is_applicable());
        assert_eq!(plan.reloadable(), ["api"]);
        assert_eq!(plan.changes[0].1.fields(), ["priority"]);
    }

    /// Every field the executor reads at bind time has to be compared, or a
    /// change to it is a reload that reports success and changes nothing.
    ///
    /// The list here is the field list of `NetworkConfig` minus `mode`, which
    /// already had a case. Deriving it would be better still; spelling it out
    /// with a test that fails when a field is added is the next best thing, and
    /// better than the state this replaces.
    #[test]
    fn every_bound_network_field_is_compared() {
        let cases: [(&str, &str, &str); 4] = [
            (
                "network.peers",
                "peers = [\"192.0.2.11\"]",
                "peers = [\"192.0.2.11\", \"192.0.2.12\"]",
            ),
            ("network.multicast", "", ""),
            (
                "network.allow_unconforming_hop_limit",
                "mode = \"unicast\"",
                "mode = \"unicast\"\nallow_unconforming_hop_limit = true",
            ),
            (
                "network.multicast",
                "mode = \"unicast\"",
                "mode = \"multicast\"",
            ),
        ];

        for (field, from, to) in cases {
            let candidate = if from.is_empty() {
                RUNNING.replace(
                    "[[instance.vip]]",
                    "[instance.network.multicast]\nttl = 100\n\n[[instance.vip]]",
                )
            } else {
                RUNNING.replace(from, to)
            };
            let running = config(RUNNING);
            let candidate_config = config(&candidate);
            let change = classify(&running.instances[0], &candidate_config.instances[0]);
            assert_ne!(
                change,
                Change::None,
                "{field} is read when the socket binds, so a change to it must be seen"
            );
        }
    }

    #[test]
    fn a_peer_change_is_seen_at_all() {
        let candidate = RUNNING.replace(
            "peers = [\"192.0.2.11\"]",
            "peers = [\"192.0.2.11\", \"192.0.2.12\"]",
        );
        let plan = plan(&config(RUNNING), &config(&candidate));

        // This assertion used to be `assert!(plan.is_applicable(), "peers are a
        // reloadable field")`, and it passed while the change was classified as
        // `Change::None` -- because `None` is applicable. The field was never
        // compared, so a peer change was invisible to the planner, and the test
        // said so in a way that read like coverage.
        assert_ne!(
            plan.changes[0].1,
            Change::None,
            "adding a peer must be seen by the planner"
        );
    }

    #[test]
    fn a_vip_change_needs_a_restart_and_names_the_field() {
        let candidate = RUNNING.replace("192.0.2.100/24", "192.0.2.101/24");
        let plan = plan(&config(RUNNING), &config(&candidate));

        assert!(
            !plan.is_applicable(),
            "I-09: a VIP change is not absorbed in place"
        );
        assert_eq!(plan.restarts().len(), 1);
        assert!(plan.refusals()[0].contains("vip"), "{}", plan.refusals()[0]);
        assert!(
            plan.refusals()[0].starts_with("api"),
            "{}",
            plan.refusals()[0]
        );
    }

    #[test]
    fn an_interface_change_needs_a_restart() {
        let candidate = RUNNING.replace("interface = \"eth0\"", "interface = \"eth1\"");
        let plan = plan(&config(RUNNING), &config(&candidate));

        assert!(!plan.is_applicable());
        assert!(
            plan.refusals()[0].contains("interface"),
            "{}",
            plan.refusals()[0]
        );
    }

    #[test]
    fn the_advertisement_interval_needs_a_restart() {
        // The running advertisement timer was armed from the old value, so
        // changing it underneath would leave the timer firing at the old rate.
        let candidate = RUNNING.replace(
            "advertisement_interval = \"1s\"",
            "advertisement_interval = \"2s\"",
        );
        let plan = plan(&config(RUNNING), &config(&candidate));

        assert!(!plan.is_applicable());
        assert!(plan.refusals()[0].contains("advertisement_interval"));
    }

    #[test]
    fn a_removed_instance_needs_a_restart() {
        let plan = plan(
            &config(RUNNING),
            &config("schema_version = 1\n[node]\nname = \"node-a\"\n"),
        );

        assert!(!plan.is_applicable());
        assert!(
            plan.refusals()[0].contains("removed"),
            "{}",
            plan.refusals()[0]
        );
    }

    #[test]
    fn a_new_instance_is_started_rather_than_restarted() {
        let candidate = format!(
            "{RUNNING}\n[[instance]]\nname = \"web\"\ninterface = \"eth0\"\nvrid = 43\n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.11\"]\n[[instance.vip]]\naddress = \"192.0.2.101/24\"\n"
        );
        let plan = plan(&config(RUNNING), &config(&candidate));

        assert!(
            plan.is_applicable(),
            "starting an instance is not a restart"
        );
        assert_eq!(plan.added(), ["web"]);
    }

    #[test]
    fn one_instance_needing_a_restart_rejects_the_whole_reload() {
        // This is the case the transaction exists for: a reload that one
        // instance could absorb is not a reason to leave another on old
        // settings.
        let candidate = format!(
            "{RUNNING}\n[[instance]]\nname = \"web\"\ninterface = \"eth0\"\nvrid = 43\n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.11\"]\n[[instance.vip]]\naddress = \"192.0.2.101/24\"\n"
        );
        let with_vip_change = candidate.replace("192.0.2.100/24", "192.0.2.102/24");
        let plan = plan(&config(RUNNING), &config(&with_vip_change));

        assert!(!plan.is_applicable());
        assert_eq!(plan.added(), ["web"], "the new instance is not the problem");
        assert_eq!(
            plan.restarts().len(),
            1,
            "the one instance that refuses rejects the reload"
        );
    }

    #[test]
    fn changes_are_reported_in_name_order() {
        let candidate = format!(
            "{RUNNING}\n[[instance]]\nname = \"aaa\"\ninterface = \"eth0\"\nvrid = 44\n[instance.network]\nmode = \"unicast\"\npeers = [\"192.0.2.11\"]\n[[instance.vip]]\naddress = \"192.0.2.101/24\"\n"
        );
        let plan = plan(&config(RUNNING), &config(&candidate));

        let names: Vec<&str> = plan.changes.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["aaa", "api"],
            "a stable order makes the report readable"
        );
    }
}
