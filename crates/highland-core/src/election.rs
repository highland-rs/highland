// Rust guideline compliant 2026-09-27

//! Election and tie-breaking.
//!
//! `SPEC.md` §12.3 specifies a four-step, deterministic order. This module is
//! that order, in one function, so that "who wins" is answered by code and by a
//! property test rather than by an implementation and a hope.

use std::fmt;
use std::net::IpAddr;

/// One router participating in an election.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The effective priority the candidate is advertising.
    pub priority: u8,
    /// The primary address of the candidate's interface, in the family of the
    /// advertisement. Used for tie-breaking.
    pub address: IpAddr,
    /// The candidate's peer-address set, used only as a last resort.
    pub peer_set: Vec<IpAddr>,
}

impl Candidate {
    /// Creates a candidate.
    #[must_use]
    pub fn new(priority: u8, address: IpAddr) -> Self {
        Self {
            priority,
            address,
            peer_set: Vec::new(),
        }
    }

    /// Returns `true` when this candidate and `other` can be told apart by
    /// priority or by address.
    ///
    /// Two candidates that agree on both cannot exist on one layer-2 segment:
    /// a router cannot hold an address another router also holds. The case is
    /// still defined, because two namespaces or two virtual links can produce
    /// it, and the answer there is [`ElectionReason::Undecidable`].
    #[must_use]
    pub fn is_distinguishable_from(&self, other: &Self) -> bool {
        self.priority != other.priority || self.address != other.address
    }

    /// Adds the candidate's peer addresses, for the last-resort comparison.
    #[must_use]
    pub fn with_peers(mut self, peers: impl IntoIterator<Item = IpAddr>) -> Self {
        self.peer_set = peers.into_iter().collect();
        self.peer_set.sort();
        self
    }
}

/// Which candidate wins, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winner {
    /// The local node wins.
    Local,
    /// The remote candidate wins.
    Remote,
}

impl Winner {
    /// Returns `true` when the local node wins.
    #[must_use]
    pub fn is_local(self) -> bool {
        matches!(self, Winner::Local)
    }
}

/// The reason a candidate won or lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElectionReason {
    /// Strictly higher effective priority.
    HigherPriority,
    /// Equal priority, resolved by the higher primary address.
    HigherAddress,
    /// Equal priority and address: the incumbent keeps the role.
    IncumbentKeepsRole,
    /// Equal priority and address with no incumbent, resolved by the smaller
    /// peer-address set. This is a pathological case and MUST be logged as
    /// `ambiguous_tie_break`.
    AmbiguousTieBreak,
    /// The two candidates are indistinguishable; the operator must decide.
    Undecidable,
}

impl fmt::Display for ElectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ElectionReason::HigherPriority => "higher_priority",
            ElectionReason::HigherAddress => "higher_address",
            ElectionReason::IncumbentKeepsRole => "incumbent_keeps_role",
            ElectionReason::AmbiguousTieBreak => "ambiguous_tie_break",
            ElectionReason::Undecidable => "undecidable",
        })
    }
}

/// The outcome of an election between two candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    /// Which candidate wins.
    pub winner: Winner,
    /// Why.
    pub reason: ElectionReason,
}

/// Which candidate, if either, already holds the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Incumbent {
    /// The local node is already master.
    Local,
    /// The remote node is already master.
    Remote,
    /// Neither node is master, for example both are starting at once.
    #[default]
    None,
}

/// Decides between two candidates, in the order given by `SPEC.md` §12.3.
///
/// # Examples
///
/// ```
/// use highland_core::election::{Candidate, ElectionReason, Incumbent, Winner, decide};
///
/// let local = Candidate::new(150, "192.0.2.10".parse().unwrap());
/// let remote = Candidate::new(100, "192.0.2.11".parse().unwrap());
///
/// let decision = decide(&local, &remote, Incumbent::None);
/// assert_eq!(decision.winner, Winner::Local);
/// assert_eq!(decision.reason, ElectionReason::HigherPriority);
/// ```
#[must_use]
pub fn decide(local: &Candidate, remote: &Candidate, incumbent: Incumbent) -> Decision {
    if local.priority != remote.priority {
        let winner = if local.priority > remote.priority {
            Winner::Local
        } else {
            Winner::Remote
        };
        return Decision {
            winner,
            reason: ElectionReason::HigherPriority,
        };
    }

    if local.address != remote.address {
        let winner = if local.address > remote.address {
            Winner::Local
        } else {
            Winner::Remote
        };
        return Decision {
            winner,
            reason: ElectionReason::HigherAddress,
        };
    }

    match incumbent {
        Incumbent::Local => {
            return Decision {
                winner: Winner::Local,
                reason: ElectionReason::IncumbentKeepsRole,
            };
        }
        Incumbent::Remote => {
            return Decision {
                winner: Winner::Remote,
                reason: ElectionReason::IncumbentKeepsRole,
            };
        }
        Incumbent::None => {}
    }

    match local.peer_set.cmp(&remote.peer_set) {
        std::cmp::Ordering::Less => Decision {
            winner: Winner::Local,
            reason: ElectionReason::AmbiguousTieBreak,
        },
        std::cmp::Ordering::Greater => Decision {
            winner: Winner::Remote,
            reason: ElectionReason::AmbiguousTieBreak,
        },
        std::cmp::Ordering::Equal => Decision {
            winner: Winner::Remote,
            reason: ElectionReason::Undecidable,
        },
    }
}

/// Returns `true` when a master must step down for a peer.
///
/// A master receiving an advertisement from a peer whose effective priority is
/// strictly greater than its own MUST step down, regardless of whether
/// preemption is enabled. `preempt` governs only whether a `BACKUP` may take
/// over an existing `MASTER` (`R-16`).
#[must_use]
pub fn must_step_down(local_effective: u8, remote_priority: u8) -> bool {
    remote_priority > local_effective
}

/// Returns `true` when a backup may preempt a master.
///
/// A peer advertising the reserved priority zero is relinquishing, not competing
/// at a low priority, so zero is never a preemption target. The node waits for
/// the master-down interval or for its own preemption timer instead.
#[must_use]
pub fn may_preempt(local_effective: u8, remote_priority: u8, preempt_enabled: bool) -> bool {
    preempt_enabled && remote_priority > 0 && remote_priority < local_effective
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(priority: u8, address: &str) -> Candidate {
        Candidate::new(
            priority,
            address.parse().expect("literal is a valid address"),
        )
    }

    #[test]
    fn the_higher_priority_wins() {
        let decision = decide(
            &candidate(150, "192.0.2.10"),
            &candidate(100, "192.0.2.99"),
            Incumbent::None,
        );
        assert_eq!(decision.winner, Winner::Local);
        assert_eq!(decision.reason, ElectionReason::HigherPriority);

        let decision = decide(
            &candidate(100, "192.0.2.99"),
            &candidate(150, "192.0.2.10"),
            Incumbent::None,
        );
        assert_eq!(decision.winner, Winner::Remote);
        assert_eq!(decision.reason, ElectionReason::HigherPriority);
    }

    #[test]
    fn equal_priority_is_resolved_by_the_higher_address() {
        let decision = decide(
            &candidate(150, "192.0.2.10"),
            &candidate(150, "192.0.2.11"),
            Incumbent::None,
        );
        assert_eq!(decision.winner, Winner::Remote);
        assert_eq!(decision.reason, ElectionReason::HigherAddress);
    }

    #[test]
    fn an_incumbent_keeps_the_role_when_nothing_distinguishes_the_candidates() {
        let decision = decide(
            &candidate(150, "192.0.2.10"),
            &candidate(150, "192.0.2.10"),
            Incumbent::Local,
        );
        assert_eq!(decision.winner, Winner::Local);
        assert_eq!(decision.reason, ElectionReason::IncumbentKeepsRole);
    }

    #[test]
    fn a_starting_node_yields_to_the_incumbent() {
        let decision = decide(
            &candidate(150, "192.0.2.10"),
            &candidate(150, "192.0.2.10"),
            Incumbent::Remote,
        );
        assert_eq!(decision.winner, Winner::Remote);
        assert_eq!(decision.reason, ElectionReason::IncumbentKeepsRole);
    }

    #[test]
    fn the_peer_set_fallback_only_applies_with_no_incumbent() {
        let local = candidate(150, "192.0.2.10");
        let remote = candidate(150, "192.0.2.10");

        let decision = decide(&local, &remote, Incumbent::None);
        assert_eq!(decision.reason, ElectionReason::Undecidable);
    }

    #[test]
    fn identical_candidates_without_an_incumbent_fall_back_to_the_peer_set() {
        let local = candidate(150, "192.0.2.10").with_peers(["192.0.2.10".parse().unwrap()]);
        let remote = candidate(150, "192.0.2.10").with_peers(["192.0.2.11".parse().unwrap()]);

        let decision = decide(&local, &remote, Incumbent::None);
        assert_eq!(decision.winner, Winner::Local);
        assert_eq!(decision.reason, ElectionReason::AmbiguousTieBreak);
    }

    #[test]
    fn candidates_that_differ_in_priority_or_address_are_distinguishable() {
        assert!(
            candidate(150, "192.0.2.10").is_distinguishable_from(&candidate(100, "192.0.2.10"))
        );
        assert!(
            candidate(150, "192.0.2.10").is_distinguishable_from(&candidate(150, "192.0.2.11"))
        );
        assert!(
            !candidate(150, "192.0.2.10").is_distinguishable_from(&candidate(150, "192.0.2.10"))
        );
    }

    #[test]
    fn two_candidates_that_cannot_be_told_apart_are_undecidable() {
        let local = Candidate::new(
            150,
            "192.0.2.10".parse().expect("literal is a valid address"),
        );
        let remote = local.clone();
        let decision = decide(&local, &remote, Incumbent::None);

        assert_eq!(decision.winner, Winner::Remote);
        assert_eq!(decision.reason, ElectionReason::Undecidable);
    }

    #[test]
    fn a_master_steps_down_only_for_a_strictly_higher_priority() {
        assert!(must_step_down(100, 150));
        assert!(!must_step_down(150, 100));
        assert!(
            !must_step_down(150, 150),
            "an equal priority is not a reason to step down"
        );
    }

    #[test]
    fn preemption_requires_the_feature_and_a_lower_non_zero_priority() {
        assert!(may_preempt(150, 100, true));
        assert!(
            !may_preempt(150, 100, false),
            "preempt = false never preempts"
        );
        assert!(
            !may_preempt(150, 0, true),
            "priority zero is a relinquish, not a lower priority"
        );
        assert!(!may_preempt(150, 150, true));
        assert!(!may_preempt(150, 200, true));
    }

    #[test]
    fn reasons_render_in_their_documented_spelling() {
        assert_eq!(
            ElectionReason::HigherPriority.to_string(),
            "higher_priority"
        );
        assert_eq!(
            ElectionReason::AmbiguousTieBreak.to_string(),
            "ambiguous_tie_break"
        );
    }
}
