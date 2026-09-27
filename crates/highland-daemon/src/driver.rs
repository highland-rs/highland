// Rust guideline compliant 2026-09-27

//! The per-instance run loop.
//!
//! One task per instance owns an [`InstanceActor`] and nothing else. The task
//! waits for one of four things: a protocol event, a timer, a health result, or
//! shutdown. It never shares state with another instance, which is what makes
//! the daemon's structure the same as the specification's (§20) rather than a
//! single supervisor with shared mutable state.
//!
//! # Time
//!
//! The loop arms one timer for the earliest deadline the state machine holds,
//! rather than one task per timer. Six timers become one `select!` arm, and the
//! deadline is recomputed after every event, so a timer that is re-armed with a
//! shorter deadline takes effect on the next iteration.

use std::time::Duration;

use highland_core::clock::Clock;
use highland_core::state::Event;
use highland_net::NetworkBackend;
use tokio::sync::mpsc;

use crate::actor::InstanceActor;

/// What the loop is asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// A protocol event, already validated by the transport.
    Event(Event),
    /// The instance's aggregate health changed.
    Health(highland_core::health::HealthSummary),
    /// A peer became reachable.
    PeerReachable,
    /// A peer became unreachable.
    PeerUnreachable,
    /// An operator paused the instance.
    Pause,
    /// An operator resumed the instance.
    Resume,
    /// An operator asked the instance to relinquish its addresses.
    Relinquish,
    /// An operator forced a role change.
    ForceRole(highland_core::state::Role),
}

impl Instruction {
    /// The instruction an operator pause sends.
    #[must_use]
    pub fn pause() -> Self {
        Instruction::Pause
    }

    /// The instruction an operator resume sends.
    #[must_use]
    pub fn resume() -> Self {
        Instruction::Resume
    }

    /// The instruction an operator relinquish sends.
    #[must_use]
    pub fn relinquish() -> Self {
        Instruction::Relinquish
    }

    /// The instruction a forced role change sends, or `None` when the role name
    /// is not one this daemon knows.
    ///
    /// A role is not accepted as a string, because `highland force-transition
    /// --role sideways` should be a refusal rather than a silent no-op.
    #[must_use]
    pub fn force(role: &str) -> Option<Self> {
        highland_core::state::Role::parse(role)
            .ok()
            .map(Instruction::ForceRole)
    }

    /// The name used in the audit record.
    #[must_use]
    pub fn operation(&self) -> &'static str {
        match self {
            Instruction::Event(_) => "event",
            Instruction::Health(_) => "health",
            Instruction::PeerReachable => "peer_reachable",
            Instruction::PeerUnreachable => "peer_unreachable",
            Instruction::Pause => "pause",
            Instruction::Resume => "resume",
            Instruction::Relinquish => "relinquish",
            Instruction::ForceRole(_) => "force_transition",
        }
    }
}

/// The channel an instance's task receives on.
pub type InstructionSender = mpsc::Sender<Instruction>;

/// The receiver an instance's task consumes.
pub type InstructionReceiver = mpsc::Receiver<Instruction>;

/// Creates the channel pair for one instance.
///
/// A small bound, because a queue that grows without limit is a denial-of-service
/// vector in the making: a flood of health results must not become an unbounded
/// backlog (`L-05`, `S-05`).
#[must_use]
pub fn channel() -> (InstructionSender, InstructionReceiver) {
    mpsc::channel(64)
}

/// How long the loop waits when the machine holds no timers at all.
const IDLE_WAIT: Duration = Duration::from_secs(1);

/// Runs one instance until `shutdown` resolves.
///
/// `C` is the clock: in production a system clock, in tests a manual one. The
/// loop only reads it, so the two are interchangeable.
pub async fn run_instance<C, B, T>(
    mut actor: InstanceActor<C, B, T>,
    mut control: InstructionReceiver,
    mut protocol: InstructionReceiver,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    registry: Option<std::sync::Arc<crate::control::StatusRegistry>>,
    name: Option<String>,
) where
    C: Clock,
    B: NetworkBackend,
    T: crate::executor::Transport,
{
    // The registry is optional so a test can run a loop without one; when it is
    // absent the machine is still the authority and simply has no reader.
    let publish = |actor: &InstanceActor<C, B, T>| {
        if let Some(registry) = &registry
            && name.is_some()
        {
            registry.publish(actor.publish_status());
        }
    };

    // The instance is told it has started, which arms the startup delay and puts
    // it in election.
    actor.handle(Event::Startup).await;
    publish(&actor);

    loop {
        if *shutdown.borrow() {
            actor.handle(Event::ShutdownRequested).await;
            return;
        }

        let wait = next_wakeup(&actor);
        tokio::select! {
            biased;

            // Shutdown is checked first so that a signal is never queued behind
            // protocol traffic.
            _ = shutdown.changed() => {
                actor.handle(Event::ShutdownRequested).await;
                publish(&actor);
                return;
            }
            // The control channel first: an operator's request is never queued
            // behind the wire.
            Some(instruction) = control.recv() => {
                apply(&mut actor, instruction).await;
                publish(&actor);
            }
            Some(instruction) = protocol.recv() => {
                apply(&mut actor, instruction).await;
                publish(&actor);
            }
            () = tokio::time::sleep(wait) => {
                let _ = actor.fire_due_timers().await;
                publish(&actor);
            }
        }
    }
}

async fn apply<C, B, T>(actor: &mut InstanceActor<C, B, T>, instruction: Instruction)
where
    C: Clock,
    B: NetworkBackend,
    T: crate::executor::Transport,
{
    let event = match instruction {
        Instruction::Event(event) => event,
        Instruction::Health(summary) => Event::HealthChanged(summary),
        // A peer becoming unreachable is not an event the machine takes. A peer
        // that stops sending is already covered by the master-down timer, and
        // inventing an event for it would give the wire a second way to change a
        // role.
        Instruction::PeerReachable | Instruction::PeerUnreachable => return,
        Instruction::Pause => Event::OperatorPauseRequested,
        Instruction::Resume => Event::OperatorResumeRequested,
        Instruction::Relinquish => Event::OperatorRelinquishRequested,
        Instruction::ForceRole(role) => Event::OperatorForceTransitionRequested {
            target: role,
            reason: "operator force-transition".to_owned(),
        },
    };
    let _ = actor.handle(event).await;
}

/// Returns how long the loop may sleep before something is due.
fn next_wakeup<C, B, T>(actor: &InstanceActor<C, B, T>) -> Duration
where
    C: Clock,
    B: NetworkBackend,
    T: crate::executor::Transport,
{
    let now = actor.machine().clock().now();
    let earliest = actor.machine().timers().deadline_of_first();
    match earliest {
        Some(deadline) => deadline
            .saturating_sub(now)
            .min(IDLE_WAIT)
            .max(Duration::from_millis(1)),
        None => IDLE_WAIT,
    }
}
