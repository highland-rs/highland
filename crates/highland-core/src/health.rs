// Rust guideline compliant 2026-09-27

//! Health aggregation and effective priority.
//!
//! The arithmetic here is the whole of `SPEC.md` §12.2, stated once, with no
//! ambiguity left for the state machine to interpret. A node that is
//! ineligible MUST NOT hold ownership, and a node whose effective priority has
//! reached zero MUST NOT transmit anything (`I-23`).

/// The policy applied when a check fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HealthPolicy {
    /// Failing blocking checks make the instance ineligible immediately.
    FailClosed,
    /// Failing checks reduce the effective priority by their weight.
    #[default]
    Weighted,
    /// Health never changes election state.
    Manual,
}

impl HealthPolicy {
    /// Returns the configuration spelling of this policy.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            HealthPolicy::FailClosed => "fail_closed",
            HealthPolicy::Weighted => "weighted",
            HealthPolicy::Manual => "manual",
        }
    }
}

/// The health configuration the state machine depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthPolicyConfig {
    /// The policy in force.
    pub policy: HealthPolicy,
    /// The floor applied to the effective priority under `weighted`.
    pub minimum_effective_priority: u8,
    /// Whether any failing check blocks ownership under `fail_closed`.
    pub all_checks_required: bool,
    /// Whether a relinquishing master announces priority zero.
    pub send_zero_priority_advert: bool,
    /// The total weight of the instance's electoral checks, for the ceiling.
    pub total_weight: u16,
}

impl Default for HealthPolicyConfig {
    fn default() -> Self {
        Self {
            policy: HealthPolicy::Weighted,
            minimum_effective_priority: 1,
            all_checks_required: false,
            send_zero_priority_advert: true,
            total_weight: 255,
        }
    }
}

/// The aggregated health of one instance.
///
/// The two failure counts are both required: `fail_closed` needs to know about
/// electoral failures alone, and `all_checks_required` needs the total. A check
/// with `weight = 0` is observational and appears only in `total_failures`
/// (`R-14`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HealthSummary {
    /// The sum of the weights of the checks currently failing.
    pub penalty: u16,
    /// The number of failing checks that can affect election.
    pub electoral_failures: usize,
    /// The number of failing checks in total, including observational ones.
    pub total_failures: usize,
    /// The number of passing checks.
    pub passing: usize,
    /// The number of results discarded as stale (`I-26`).
    pub stale_discarded: usize,
}

impl HealthSummary {
    /// Returns a summary with nothing failing.
    #[must_use]
    pub fn healthy() -> Self {
        Self::default()
    }

    /// Returns `true` when no check is failing.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.total_failures == 0
    }
}

/// The result of evaluating health against configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorityState {
    /// The configured priority.
    pub configured: u8,
    /// The penalty applied by failing checks.
    pub health_penalty: u16,
    /// The effective priority, which is never above the configured priority.
    pub effective: u8,
    /// Whether the instance may hold or contest ownership.
    pub eligible: bool,
}

impl PriorityState {
    /// Returns the effective priority as it appears on the wire.
    ///
    /// An effective priority of zero is internal only: the node relinquishes
    /// silently instead of transmitting a reserved value (`I-23`, `I-27`).
    #[must_use]
    pub fn advert_priority(self) -> u8 {
        self.effective
    }
}

/// Evaluates health against configuration.
///
/// The arithmetic is exactly as specified:
///
/// ```text
/// weighted:  effective = max(minimum_effective_priority, configured - penalty)
/// fail_closed: eligible unless a blocking check is failing
/// manual:    eligible, and effective = configured
/// ```
///
/// # Examples
///
/// ```
/// use highland_core::health::{HealthPolicyConfig, HealthSummary, evaluate};
///
/// let config = HealthPolicyConfig { minimum_effective_priority: 100, ..HealthPolicyConfig::default() };
/// let summary = HealthSummary { penalty: 50, ..HealthSummary::default() };
/// let state = evaluate(150, &config, &summary);
/// assert_eq!(state.effective, 100);
/// assert!(state.eligible);
/// ```
#[must_use]
pub fn evaluate(
    configured: u8,
    config: &HealthPolicyConfig,
    health: &HealthSummary,
) -> PriorityState {
    match config.policy {
        HealthPolicy::Manual => PriorityState {
            configured,
            health_penalty: 0,
            effective: configured,
            eligible: configured > 0,
        },
        HealthPolicy::Weighted => {
            let effective = subtract_weight(
                configured,
                health.penalty,
                config.minimum_effective_priority,
            );
            PriorityState {
                configured,
                health_penalty: health.penalty,
                effective,
                eligible: effective > 0,
            }
        }
        HealthPolicy::FailClosed => {
            let blocking = if config.all_checks_required {
                health.total_failures
            } else {
                health.electoral_failures
            };
            PriorityState {
                configured,
                health_penalty: health.penalty,
                effective: configured,
                eligible: blocking == 0 && configured > 0,
            }
        }
    }
}

/// Returns `min(configured, max(floor, configured - penalty))`.
///
/// The floor is a floor, not a target: a floor above the configured priority
/// would otherwise raise the effective priority, which `I-24` forbids. Both
/// ends saturate rather than wrap.
fn subtract_weight(configured: u8, penalty: u16, floor: u8) -> u8 {
    let reduced = u32::from(configured).saturating_sub(u32::from(penalty));
    let floored = reduced.max(u32::from(floor));
    let capped = floored.min(u32::from(configured));
    u8::try_from(capped).unwrap_or(configured)
}

/// Returns `true` when a failure under this configuration requires an immediate
/// relinquish, as opposed to a priority reduction.
///
/// Only `fail_closed` does. A `weighted` demotion is expressed through the
/// effective priority and the step-down rule (`R-16`), not through a relinquish.
#[must_use]
pub fn requires_immediate_relinquish(config: &HealthPolicyConfig, health: &HealthSummary) -> bool {
    if config.policy != HealthPolicy::FailClosed {
        return false;
    }
    let blocking = if config.all_checks_required {
        health.total_failures
    } else {
        health.electoral_failures
    };
    blocking > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weighted(minimum: u8) -> HealthPolicyConfig {
        HealthPolicyConfig {
            policy: HealthPolicy::Weighted,
            minimum_effective_priority: minimum,
            ..HealthPolicyConfig::default()
        }
    }

    fn fail_closed(all_required: bool) -> HealthPolicyConfig {
        HealthPolicyConfig {
            policy: HealthPolicy::FailClosed,
            all_checks_required: all_required,
            ..HealthPolicyConfig::default()
        }
    }

    #[test]
    fn healthy_weighted_health_leaves_the_priority_untouched() {
        let state = evaluate(150, &weighted(1), &HealthSummary::healthy());
        assert_eq!(
            state,
            PriorityState {
                configured: 150,
                health_penalty: 0,
                effective: 150,
                eligible: true
            }
        );
    }

    #[test]
    fn weighted_penalty_reduces_the_effective_priority() {
        let summary = HealthSummary {
            penalty: 50,
            ..HealthSummary::default()
        };
        assert_eq!(evaluate(150, &weighted(1), &summary).effective, 100);
        assert_eq!(evaluate(150, &weighted(1), &summary).health_penalty, 50);
    }

    #[test]
    fn the_effective_priority_never_falls_below_the_floor() {
        let summary = HealthSummary {
            penalty: 255,
            ..HealthSummary::default()
        };
        let state = evaluate(150, &weighted(100), &summary);
        assert_eq!(state.effective, 100);
        assert!(state.eligible);
    }

    #[test]
    fn an_effective_priority_of_zero_makes_the_node_ineligible() {
        let config = weighted(0);
        let summary = HealthSummary {
            penalty: 200,
            ..HealthSummary::default()
        };
        let state = evaluate(150, &config, &summary);

        assert_eq!(state.effective, 0);
        assert!(
            !state.eligible,
            "I-23: an effective priority of zero means not master"
        );
    }

    #[test]
    fn a_floor_above_the_configured_priority_never_raises_it() {
        let summary = HealthSummary {
            total_failures: 3,
            electoral_failures: 2,
            ..HealthSummary::default()
        };
        let state = evaluate(150, &weighted(200), &summary);
        assert_eq!(
            state.effective, 150,
            "I-24: the floor is a floor, not a target"
        );
    }

    #[test]
    fn the_effective_priority_never_exceeds_the_configured_priority() {
        let summary = HealthSummary {
            total_failures: 3,
            electoral_failures: 2,
            ..HealthSummary::default()
        };
        assert!(evaluate(150, &weighted(1), &summary).effective <= 150);
        assert!(evaluate(150, &fail_closed(true), &summary).effective <= 150);
        assert!(evaluate(150, &weighted(200), &summary).effective <= 150);
    }

    #[test]
    fn manual_health_never_changes_election_state() {
        let config = HealthPolicyConfig {
            policy: HealthPolicy::Manual,
            ..HealthPolicyConfig::default()
        };
        let summary = HealthSummary {
            penalty: 255,
            electoral_failures: 2,
            total_failures: 2,
            passing: 0,
            stale_discarded: 0,
        };
        let state = evaluate(150, &config, &summary);

        assert_eq!(state.effective, 150);
        assert!(state.eligible);
    }

    #[test]
    fn fail_closed_ignores_observational_failures_by_default() {
        let summary = HealthSummary {
            total_failures: 1,
            electoral_failures: 0,
            ..HealthSummary::default()
        };
        assert!(evaluate(150, &fail_closed(false), &summary).eligible);
        assert!(!evaluate(150, &fail_closed(true), &summary).eligible);
    }

    #[test]
    fn fail_closed_blocks_on_an_electoral_failure() {
        let summary = HealthSummary {
            total_failures: 1,
            electoral_failures: 1,
            ..HealthSummary::default()
        };
        assert!(!evaluate(150, &fail_closed(false), &summary).eligible);
        assert!(requires_immediate_relinquish(&fail_closed(false), &summary));
    }

    #[test]
    fn weighted_demotion_does_not_relinquish_immediately() {
        let summary = HealthSummary {
            penalty: 100,
            electoral_failures: 1,
            total_failures: 1,
            ..HealthSummary::default()
        };
        assert!(!requires_immediate_relinquish(&weighted(1), &summary));
    }

    #[test]
    fn policies_render_in_their_configuration_spelling() {
        assert_eq!(HealthPolicy::FailClosed.as_str(), "fail_closed");
        assert_eq!(HealthPolicy::Weighted.as_str(), "weighted");
        assert_eq!(HealthPolicy::Manual.as_str(), "manual");
    }
}
