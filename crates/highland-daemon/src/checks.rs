// Rust guideline compliant 2026-09-27

// On a platform with no instances there is nothing to built from configuration, and the
// only caller of this module is the Linux-only per-instance path.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! Building probes out of configuration.
//!
//! The configuration layer validates a check; this turns one into something that
//! can be run. It is the seam where a typo in a configuration file becomes a
//! refusal at startup rather than a failing check on every interval forever —
//! which matters, because a check that always fails and costs priority is a
//! quieter way to take a node out of service than a refusal.
//!
//! Only the types that are implemented are built. The rest are refused by name,
//! with the reason, so a configuration that asks for something this build
//! cannot do honestly fails loudly instead of appearing to work.

use std::sync::Arc;

use highland_checks::{
    Check, CheckError, CheckKind, CheckSpec, HttpCheck, InterfaceCheck, LinkProbe, Result,
    TcpCheck, UnixCheck,
};

use crate::options::CheckPlan;

/// The kind a configuration's `type` names, or the reason it does not name one
/// this build has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeKind {
    /// A TCP connect.
    Tcp,
    /// An HTTP request.
    Http,
    /// A Unix socket connect.
    Unix,
    /// An interface or carrier probe.
    Interface,
    /// A type this build does not implement, named for the message.
    Unsupported(&'static str),
}

/// Returns the kind a configuration's `type` names.
///
/// The names are matched exactly and case-sensitively, because a check type is a
/// protocol name rather than prose, and a spelling that almost matches is a
/// mistake worth reporting rather than guessing at.
#[must_use]
pub(crate) fn kind_of(configured: &str) -> ProbeKind {
    // The reasons come from `highland_config`, which is the same table
    // validation reads. A second copy of these strings here would be a second
    // answer to "can this build run this check", and the two drifting apart is
    // how a `check-config` that passes becomes a daemon that quietly ignores a
    // health gate.
    match configured {
        "tcp" => ProbeKind::Tcp,
        "http" => ProbeKind::Http,
        "unix" => ProbeKind::Unix,
        "interface" => ProbeKind::Interface,
        other => {
            ProbeKind::Unsupported(highland_config::unimplemented_check_reason(other).unwrap_or(""))
        }
    }
}

/// Builds a runnable check from its plan.
///
/// `links` is how an interface check asks the kernel about a link; the caller
/// supplies it so this module needs no platform code and a test needs no kernel.
///
/// # Errors
///
/// Returns [`CheckError`] when the plan's type is not implemented here, when a
/// required key is missing, or when a target cannot be parsed. All three are
/// configuration errors, and all three are reported before the daemon starts.
/// Builds every probe a plan list describes, or says which one cannot be built.
///
/// The runner uses this as a gate *before* an instance is created, so an instance
/// whose checks cannot run never participates rather than participating without
/// them. Returning the probes also means the health task does not build them a
/// second time, which is what keeps that gate and the running checks from
/// disagreeing about which types work.
///
/// # Errors
///
/// Returns the first failure, naming the check and the reason, so the message an
/// operator reads is the one that says what to change.
pub(crate) fn build_all(
    plans: &[CheckPlan],
    links: &Arc<dyn LinkProbe>,
) -> std::result::Result<Vec<Arc<dyn Check>>, String> {
    let mut checks = Vec::with_capacity(plans.len());
    for plan in plans {
        match build(plan, Arc::clone(links)) {
            Ok(check) => checks.push(check),
            Err(error) => {
                return Err(format!("check {} cannot be built: {error}", plan.name));
            }
        }
    }
    Ok(checks)
}

/// Builds a runnable check from a plan.
///
/// # Errors
///
/// Returns [`CheckError`] when the plan's type is not implemented here, when a
/// required key is missing, or when a target cannot be parsed. All three are
/// configuration errors, and all three are reported before the daemon starts.
pub(crate) fn build(plan: &CheckPlan, links: Arc<dyn LinkProbe>) -> Result<Arc<dyn Check>> {
    let spec = spec_of(plan)?;
    match kind_of(&plan.check_type) {
        ProbeKind::Tcp => {
            let address = plan
                .address
                .as_deref()
                .ok_or_else(|| CheckError::MissingKey {
                    check: plan.name.clone(),
                    key: "address",
                })?;
            Ok(Arc::new(TcpCheck::new(spec, address)?))
        }
        ProbeKind::Http => {
            let url = plan.url.as_deref().ok_or_else(|| CheckError::MissingKey {
                check: plan.name.clone(),
                key: "url",
            })?;
            let expected = plan.expected_status.clone().unwrap_or_default();
            Ok(Arc::new(HttpCheck::new(spec, url, expected)?))
        }
        ProbeKind::Unix => {
            let path = plan.path.as_deref().ok_or_else(|| CheckError::MissingKey {
                check: plan.name.clone(),
                key: "path",
            })?;
            Ok(Arc::new(UnixCheck::new(spec, path)))
        }
        ProbeKind::Interface => {
            let interface = plan
                .interface
                .as_deref()
                .or(plan.address.as_deref())
                .ok_or_else(|| CheckError::MissingKey {
                    check: plan.name.clone(),
                    key: "interface",
                })?;
            Ok(Arc::new(InterfaceCheck::new(
                spec,
                interface,
                plan.expected_address.clone(),
                links,
            )))
        }
        ProbeKind::Unsupported(detail) => Err(CheckError::UnresolvableTarget {
            check: plan.name.clone(),
            target: plan.check_type.clone(),
            detail: if detail.is_empty() {
                format!("{} is not a check type this build knows", plan.check_type)
            } else {
                detail.to_owned()
            },
        }),
    }
}

/// Builds the specification a plan describes.
fn spec_of(plan: &CheckPlan) -> Result<CheckSpec> {
    CheckSpec::new(
        plan.name.clone(),
        kind_to_check_kind(&plan.check_type),
        plan.interval,
        plan.timeout,
        plan.failure_threshold,
        plan.success_threshold,
        plan.weight,
    )
    .map(|mut spec| {
        spec.initial_grace_period = plan.initial_grace_period;
        spec
    })
}

/// The specification's own kind, which is only used for the check's identity.
fn kind_to_check_kind(configured: &str) -> CheckKind {
    match kind_of(configured) {
        ProbeKind::Tcp => CheckKind::Tcp,
        ProbeKind::Http | ProbeKind::Unsupported(_) => CheckKind::Http,
        ProbeKind::Unix => CheckKind::Unix,
        ProbeKind::Interface => CheckKind::Interface,
    }
}
