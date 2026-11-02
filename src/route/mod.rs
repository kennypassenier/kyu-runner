//! The pump: one route, one loop, one in-flight message (AR2). The
//! failure semantics of AR3/AR5/AR6/AR15/AR16 live here and nowhere
//! else. Payloads never reach a log line (AR14) — ids, sizes and
//! statuses only.
//!
//! Layout: this file holds the runner, its loop and the loop's state;
//! `outcome` decides what each poll result means, `deliver` delivers and
//! settles one claim, `policy` pushes the W3 policy, and `backoff` holds
//! the waiting primitives.

mod backoff;
mod deliver;
mod outcome;
mod policy;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tracing::{debug, info};

use crate::config::Route;
use crate::health::HealthState;
use crate::hub::{HubClient, PollOutcome};
use crate::webhook::WebhookClient;
use backoff::Backoff;

#[derive(Clone)]
pub struct RouteRunner {
    pub route: Route,
    pub hub: Arc<HubClient>,
    pub webhook: Arc<WebhookClient>,
    pub webhook_timeout: Duration,
    pub lease_budget: Duration,
    /// W3: the `[routes.policy]` block as hub JSON, applied after the
    /// first successful poll (a policy PUT on a subscription that does
    /// not exist yet fails — the poll is what creates it).
    pub policy_json: Option<String>,
    pub health: Arc<HealthState>,
    /// MR1: operational timings from `[tuning]`, defaults unchanged.
    pub hub_backoff: (u64, u64),
    pub circuit_backoff: (u64, u64),
    pub circuit_probe_timeout: Duration,
    pub topic_unborn_poll: Duration,
}

/// Whether the route loop goes on after handling one poll outcome.
enum Flow {
    Continue,
    Stop,
}

impl Flow {
    fn stop_if(shutdown: bool) -> Self {
        if shutdown { Flow::Stop } else { Flow::Continue }
    }
}

/// Everything the loop remembers between polls.
struct LoopState {
    /// AR16: set after a 404 — the topic has not been born yet, so the
    /// poll that eventually creates our subscription must carry
    /// from=beginning or the birth messages fall in the gap.
    replay: bool,
    unborn_logged: bool,
    hub_down: bool,
    auth_denied: bool,
    hub_backoff: Backoff,
    circuit_backoff: Backoff,
    policy_pending: bool,
    policy_warned: bool,
}

impl LoopState {
    fn new(runner: &RouteRunner) -> Self {
        Self {
            replay: false,
            unborn_logged: false,
            hub_down: false,
            auth_denied: false,
            hub_backoff: Backoff::new(runner.hub_backoff.0, runner.hub_backoff.1),
            circuit_backoff: Backoff::new(runner.circuit_backoff.0, runner.circuit_backoff.1),
            policy_pending: runner.policy_json.is_some(),
            policy_warned: false,
        }
    }

    /// The hub answered normally: log the recovery once and reset the
    /// hub backoff.
    fn note_recovery(&mut self, name: &str) {
        if self.hub_down || self.auth_denied {
            info!(route = %name, "hub answering normally again — resuming");
        }
        self.hub_down = false;
        self.auth_denied = false;
        self.hub_backoff.reset();
    }

    /// The topic exists: no replay needed, and a later 404 logs again.
    fn note_topic_alive(&mut self) {
        self.replay = false;
        self.unborn_logged = false;
    }
}

impl RouteRunner {
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let name = self.route.name.clone();
        let mut state = LoopState::new(&self);
        while !*shutdown.borrow() {
            let outcome = tokio::select! {
                biased;
                _ = shutdown.changed() => continue,
                outcome = self.hub.next(&self.route.topic, &self.route.subscription, state.replay) => outcome,
            };
            let flow = match outcome {
                Err(error) => {
                    self.on_hub_error(&name, error, &mut state, &mut shutdown)
                        .await
                }
                Ok(PollOutcome::TopicUnborn) => {
                    self.on_topic_unborn(&name, &mut state, &mut shutdown).await
                }
                Ok(PollOutcome::Empty) => {
                    self.on_empty(&name, &mut state).await;
                    Flow::Continue
                }
                Ok(PollOutcome::Oversize {
                    id,
                    at_least,
                    attempt,
                }) => {
                    self.on_oversize(&name, &id, at_least, attempt.as_deref(), &mut state)
                        .await;
                    Flow::Continue
                }
                Ok(PollOutcome::Message(message)) => {
                    self.on_message(&name, message, &mut state, &mut shutdown)
                        .await
                }
            };
            if matches!(flow, Flow::Stop) {
                break;
            }
        }
        self.health.set(&name, "stopped");
        debug!(route = %name, "route loop stopped");
    }
}
