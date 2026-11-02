//! What the route loop does with each poll outcome. Every handler
//! returns a `Flow`: keep looping, or stop because shutdown arrived
//! while it was waiting.

use tokio::sync::watch;
use tracing::{info, warn};

use super::backoff::interrupted;
use super::deliver::DeliverEnd;
use super::{Flow, LoopState, RouteRunner};
use crate::hub::{Claimed, HubError};
use crate::webhook;

impl RouteRunner {
    /// The hub refused or could not be reached. Both log on transition
    /// only (AR6), then wait out the hub backoff.
    pub(super) async fn on_hub_error(
        &self,
        name: &str,
        error: HubError,
        state: &mut LoopState,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Flow {
        if matches!(error, HubError::Auth) {
            // AR6: auth joins transition-only logging — the hub is up,
            // so this must not flood every poll.
            if !state.auth_denied {
                warn!(route = %name, "{}", HubError::Auth);
                state.auth_denied = true;
            }
            self.health.set(name, "auth-denied");
        } else {
            if !state.hub_down {
                warn!(
                    route = %name, %error,
                    "hub unreachable — reconnecting with backoff; nothing is lost, the \
                     backlog waits on the hub (K7)"
                );
                state.hub_down = true;
            }
            self.health.set(name, "hub-down");
        }
        Flow::stop_if(interrupted(state.hub_backoff.next(), shutdown).await)
    }

    /// AR16: the topic does not exist yet. Wait quietly and remember to
    /// replay from the beginning once it is born.
    pub(super) async fn on_topic_unborn(
        &self,
        name: &str,
        state: &mut LoopState,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Flow {
        state.note_recovery(name);
        self.health.set(name, "topic-unborn");
        if !state.unborn_logged {
            info!(
                route = %name, topic = %self.route.topic,
                "topic not born yet (kyu creates topics on first publish) — waiting quietly; \
                 its first messages will be replayed from the beginning once it appears (AR16)"
            );
            state.unborn_logged = true;
        }
        state.replay = true;
        Flow::stop_if(interrupted(self.topic_unborn_poll, shutdown).await)
    }

    pub(super) async fn on_empty(&self, name: &str, state: &mut LoopState) {
        state.note_recovery(name);
        state.note_topic_alive();
        self.apply_policy(name, state).await;
        self.health.set(name, "idle");
    }

    /// Security F1: a message over the body cap is nacked unread.
    /// Bounded: at most once per delivery attempt, and the attempts run
    /// out into a visible dead letter.
    pub(super) async fn on_oversize(
        &self,
        name: &str,
        id: &str,
        at_least: usize,
        state: &mut LoopState,
    ) {
        state.note_recovery(name);
        warn!(
            route = %name, id = %id, at_least_bytes = at_least,
            "message larger than max_body_bytes — nacked without delivery; it will dead-letter \
             visibly when its attempts run out (security F1)"
        );
        self.settle_nack(name, id, None).await;
    }

    pub(super) async fn on_message(
        &self,
        name: &str,
        message: Box<Claimed>,
        state: &mut LoopState,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Flow {
        state.note_recovery(name);
        state.note_topic_alive();
        self.apply_policy(name, state).await;
        self.health.set(name, "delivering");
        panic_drill(name);
        match self.deliver_with_budget(name, &message, shutdown).await {
            DeliverEnd::Delivered => {
                self.settle_ack(name, &message).await;
                self.health.count_delivered(name);
                self.health.set(name, "idle");
                Flow::Continue
            }
            DeliverEnd::Shutdown => {
                self.settle_nack(name, &message.id, message.attempt()).await;
                Flow::Stop
            }
            DeliverEnd::GiveUp { connect_class } => {
                self.settle_nack(name, &message.id, message.attempt()).await;
                if !connect_class {
                    return Flow::Continue;
                }
                self.wait_out_circuit(name, state, shutdown).await
            }
        }
    }

    /// AR15: stop claiming so the backlog accumulates unclaimed — no
    /// attempts burn while the target machine is down, and recovery
    /// drains in publish order. Probe the target until it answers.
    async fn wait_out_circuit(
        &self,
        name: &str,
        state: &mut LoopState,
        shutdown: &mut watch::Receiver<bool>,
    ) -> Flow {
        warn!(
            route = %name,
            "webhook target unreachable — circuit open: claiming is paused, the backlog \
             accumulates unclaimed on the hub (AR15)"
        );
        self.health.set(name, "circuit-open");
        state.circuit_backoff.reset();
        loop {
            if interrupted(state.circuit_backoff.next(), shutdown).await {
                return Flow::Stop;
            }
            if webhook::probe_origin(&self.route.webhook_url, self.circuit_probe_timeout).await {
                info!(
                    route = %name,
                    "webhook target reachable again — circuit closed, resuming delivery"
                );
                return Flow::Continue;
            }
        }
    }
}

/// AR1 supervision drill hook: debug builds only, so the release binary
/// cannot carry the trigger. The supervisor must respawn the loop; the
/// unacked claim redelivers (K5).
#[cfg(debug_assertions)]
fn panic_drill(name: &str) {
    const DRILL_ENV: &str = "KYU_RUNNER_TEST_PANIC_ROUTE";
    if std::env::var(DRILL_ENV).as_deref() == Ok(name) {
        // SAFETY: test-only hook; the drill sets the variable before
        // start and nothing else reads it concurrently.
        unsafe { std::env::remove_var(DRILL_ENV) };
        panic!("test-injected route panic (AR1 drill)");
    }
}

#[cfg(not(debug_assertions))]
fn panic_drill(_name: &str) {}
