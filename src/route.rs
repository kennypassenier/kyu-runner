//! The pump: one route, one loop, one in-flight message (AR2). The
//! failure semantics of AR3/AR5/AR6/AR15/AR16 live here and nowhere
//! else. Payloads never reach a log line (AR14) — ids, sizes and
//! statuses only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tracing::{debug, info, warn};

use crate::config::Route;
use crate::health::HealthState;
use crate::hub::{Claimed, HubClient, HubError, PollOutcome};
use crate::webhook::{self, DeliveryError, WebhookClient};

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
}

enum DeliverEnd {
    Delivered,
    GiveUp { connect_class: bool },
    Shutdown,
}

/// Exponential backoff. The jitter comes from the nanosecond clock —
/// enough to spread retries, no rand dependency.
struct Backoff {
    next_ms: u64,
    base_ms: u64,
    cap_ms: u64,
}

impl Backoff {
    fn new(base_ms: u64, cap_ms: u64) -> Self {
        Self {
            next_ms: base_ms,
            base_ms,
            cap_ms,
        }
    }

    fn next(&mut self) -> Duration {
        let ms = self.next_ms;
        self.next_ms = (self.next_ms * 2).min(self.cap_ms);
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| u64::from(now.subsec_nanos()))
            .unwrap_or(0)
            % (ms / 8 + 1);
        Duration::from_millis(ms + jitter)
    }

    fn reset(&mut self) {
        self.next_ms = self.base_ms;
    }
}

/// Sleep that a shutdown signal cuts short. Returns true on shutdown.
async fn interrupted(delay: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
    if *shutdown.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => false,
        _ = shutdown.changed() => true,
    }
}

impl RouteRunner {
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let name = self.route.name.clone();
        let topic = self.route.topic.clone();
        let subscription = self.route.subscription.clone();

        // AR16: set after a 404 — the topic has not been born yet, so
        // the poll that eventually creates our subscription must carry
        // from=beginning or the birth messages fall in the gap.
        let mut replay = false;
        let mut unborn_logged = false;
        let mut hub_down = false;
        let mut auth_denied = false;
        let mut hub_backoff = Backoff::new(500, 30_000);
        let mut circuit_backoff = Backoff::new(1_000, 60_000);
        let mut policy_pending = self.policy_json.is_some();
        let mut policy_warned = false;

        'route: loop {
            if *shutdown.borrow() {
                break;
            }
            let outcome = tokio::select! {
                biased;
                _ = shutdown.changed() => continue,
                outcome = self.hub.next(&topic, &subscription, replay) => outcome,
            };
            match outcome {
                Err(HubError::Auth) => {
                    // AR6: auth joins transition-only logging — the hub
                    // is up, so this must not flood every poll.
                    if !auth_denied {
                        warn!(route = %name, "{}", HubError::Auth);
                        auth_denied = true;
                    }
                    self.health.set(&name, "auth-denied");
                    if interrupted(hub_backoff.next(), &mut shutdown).await {
                        break;
                    }
                }
                Err(error) => {
                    if !hub_down {
                        warn!(
                            route = %name, %error,
                            "hub unreachable — reconnecting with backoff; nothing is lost, the \
                             backlog waits on the hub (K7)"
                        );
                        hub_down = true;
                    }
                    self.health.set(&name, "hub-down");
                    if interrupted(hub_backoff.next(), &mut shutdown).await {
                        break;
                    }
                }
                Ok(PollOutcome::TopicUnborn) => {
                    self.note_recovery(&name, &mut hub_down, &mut auth_denied, &mut hub_backoff);
                    self.health.set(&name, "topic-unborn");
                    if !unborn_logged {
                        info!(
                            route = %name, topic = %topic,
                            "topic not born yet (kyu creates topics on first publish) — \
                             waiting quietly; its first messages will be replayed from the \
                             beginning once it appears (AR16)"
                        );
                        unborn_logged = true;
                    }
                    replay = true;
                    if interrupted(Duration::from_secs(5), &mut shutdown).await {
                        break;
                    }
                }
                Ok(PollOutcome::Empty) => {
                    self.note_recovery(&name, &mut hub_down, &mut auth_denied, &mut hub_backoff);
                    replay = false;
                    unborn_logged = false;
                    self.apply_policy(&name, &mut policy_pending, &mut policy_warned)
                        .await;
                    self.health.set(&name, "idle");
                }
                Ok(PollOutcome::Oversize { id, at_least }) => {
                    self.note_recovery(&name, &mut hub_down, &mut auth_denied, &mut hub_backoff);
                    // Bounded: at most once per delivery attempt, and
                    // the attempts run out into a visible dead letter.
                    warn!(
                        route = %name, id = %id, at_least_bytes = at_least,
                        "message larger than max_body_bytes — nacked without delivery; it will \
                         dead-letter visibly when its attempts run out (security F1)"
                    );
                    self.settle_nack(&name, &id).await;
                }
                Ok(PollOutcome::Message(message)) => {
                    self.note_recovery(&name, &mut hub_down, &mut auth_denied, &mut hub_backoff);
                    replay = false;
                    unborn_logged = false;
                    self.apply_policy(&name, &mut policy_pending, &mut policy_warned)
                        .await;
                    self.health.set(&name, "delivering");
                    // AR1 supervision drill hook: debug builds only, so
                    // the release binary cannot carry the trigger.
                    #[cfg(debug_assertions)]
                    if std::env::var("HUB_BRIDGE_TEST_PANIC_ROUTE").as_deref() == Ok(name.as_str())
                    {
                        // The supervisor must respawn the loop; the
                        // unacked claim redelivers (K5).
                        unsafe { std::env::remove_var("HUB_BRIDGE_TEST_PANIC_ROUTE") };
                        panic!("test-injected route panic (AR1 drill)");
                    }
                    match self
                        .deliver_with_budget(&name, &message, &mut shutdown)
                        .await
                    {
                        DeliverEnd::Delivered => {
                            self.settle_ack(&name, &message).await;
                            self.health.count_delivered(&name);
                            self.health.set(&name, "idle");
                        }
                        DeliverEnd::Shutdown => {
                            self.settle_nack(&name, &message.id).await;
                            break;
                        }
                        DeliverEnd::GiveUp { connect_class } => {
                            self.settle_nack(&name, &message.id).await;
                            if connect_class {
                                // AR15: stop claiming so the backlog
                                // accumulates unclaimed — no attempts
                                // burn while the target machine is down,
                                // and recovery drains in publish order.
                                warn!(
                                    route = %name,
                                    "webhook target unreachable — circuit open: claiming is \
                                     paused, the backlog accumulates unclaimed on the hub (AR15)"
                                );
                                self.health.set(&name, "circuit-open");
                                circuit_backoff.reset();
                                loop {
                                    if interrupted(circuit_backoff.next(), &mut shutdown).await {
                                        break 'route;
                                    }
                                    if webhook::probe_origin(&self.route.webhook_url).await {
                                        info!(
                                            route = %name,
                                            "webhook target reachable again — circuit closed, \
                                             resuming delivery"
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        self.health.set(&name, "stopped");
        debug!(route = %name, "route loop stopped");
    }

    /// W3: the poll that just succeeded proved the subscription exists,
    /// so the PUT can land now. Failures retry on later iterations —
    /// warn once, then quiet (AR6). The very first claim of a fresh
    /// subscription still runs under the hub-default lease; a budget
    /// computed from a larger configured lease can then outlive that
    /// one claim under sustained failure, which at worst costs one
    /// legal duplicate (at-least-once).
    async fn apply_policy(&self, name: &str, pending: &mut bool, warned: &mut bool) {
        if !*pending {
            return;
        }
        let Some(policy_json) = &self.policy_json else {
            *pending = false;
            return;
        };
        match self
            .hub
            .put_policy(&self.route.topic, &self.route.subscription, policy_json)
            .await
        {
            Ok(in_force) => {
                // Security F4: hub-controlled text, sanitized for the log.
                info!(
                    route = %name,
                    in_force = %crate::hub::printable(in_force.trim(), 600),
                    "policy in force (W3) — the hub's answer is authoritative; note that a \
                     policy write replaces every field, so dashboard tweaks are reverted here"
                );
                *pending = false;
            }
            Err(error) => {
                if !*warned {
                    warn!(
                        route = %name, %error,
                        "policy PUT failed — retrying on later polls; until it lands the hub's \
                         current policy governs this subscription (W3)"
                    );
                    *warned = true;
                } else {
                    debug!(route = %name, %error, "policy PUT still failing");
                }
            }
        }
    }

    fn note_recovery(
        &self,
        name: &str,
        hub_down: &mut bool,
        auth_denied: &mut bool,
        hub_backoff: &mut Backoff,
    ) {
        if *hub_down || *auth_denied {
            info!(route = %name, "hub answering normally again — resuming");
        }
        *hub_down = false;
        *auth_denied = false;
        hub_backoff.reset();
    }

    /// AR3: retry in-process on the same claim while the lease budget
    /// allows — the head of the line stays the head of the line (AR2).
    async fn deliver_with_budget(
        &self,
        name: &str,
        message: &Claimed,
        shutdown: &mut watch::Receiver<bool>,
    ) -> DeliverEnd {
        let start = Instant::now();
        let mut delay = Duration::from_secs(1);
        loop {
            match self
                .webhook
                .deliver(&self.route.webhook_url, self.webhook_timeout, message)
                .await
            {
                Ok(()) => return DeliverEnd::Delivered,
                Err(error) => {
                    let connect_class = matches!(error, DeliveryError::ConnectClass(_));
                    if start.elapsed() + delay + self.webhook_timeout > self.lease_budget {
                        if !connect_class {
                            // Bounded: at most one line per lease budget
                            // per route (AR6's no-flood property).
                            warn!(
                                route = %name, id = %message.id, %error,
                                "delivery kept failing within the lease budget — nacking; the \
                                 hub's retry and dead-letter machinery takes over (K4)"
                            );
                        }
                        return DeliverEnd::GiveUp { connect_class };
                    }
                    debug!(
                        route = %name, id = %message.id, %error,
                        "delivery failed — retrying in-process on the same claim (AR3)"
                    );
                    if interrupted(delay, shutdown).await {
                        return DeliverEnd::Shutdown;
                    }
                    delay = (delay * 2).min(Duration::from_secs(8));
                }
            }
        }
    }

    async fn settle_ack(&self, name: &str, message: &Claimed) {
        for attempt in 0..2u8 {
            match self
                .hub
                .ack(&self.route.topic, &self.route.subscription, &message.id)
                .await
            {
                Ok(()) => {
                    debug!(route = %name, id = %message.id, bytes = message.body.len(),
                        "delivered and acked");
                    return;
                }
                Err(error) if attempt == 0 => {
                    debug!(route = %name, id = %message.id, %error, "ack failed — one retry");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(error) => {
                    // A lost ack means a redelivery; the duplicate is
                    // legal by contract (S3, AR13).
                    warn!(
                        route = %name, id = %message.id, %error,
                        "ack failed after a retry — the message will redeliver; consumers \
                         tolerate the duplicate (at-least-once, AR13)"
                    );
                }
            }
        }
    }

    async fn settle_nack(&self, name: &str, id: &str) {
        self.health.count_nacked(name);
        if let Err(error) = self
            .hub
            .nack(&self.route.topic, &self.route.subscription, id)
            .await
        {
            debug!(
                route = %name, id = %id, %error,
                "nack failed — the lease will expire and redeliver on its own (kyu K5)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Backoff;

    #[test]
    fn l3_ar6_backoff_doubles_to_its_cap_with_bounded_jitter() {
        let mut backoff = Backoff::new(500, 30_000);
        let mut expected = 500u64;
        for _ in 0..10 {
            let delay = backoff.next().as_millis() as u64;
            // Jitter adds at most an eighth of the base step.
            assert!(
                delay >= expected && delay <= expected + expected / 8 + 1,
                "delay {delay} outside [{expected}, +12.5%]"
            );
            expected = (expected * 2).min(30_000);
        }
        assert_eq!(expected, 30_000, "the cap is reached and held");
    }

    #[test]
    fn l3_ar6_reset_returns_to_the_base() {
        let mut backoff = Backoff::new(1_000, 60_000);
        for _ in 0..8 {
            backoff.next();
        }
        backoff.reset();
        let delay = backoff.next().as_millis() as u64;
        assert!((1_000..=1_126).contains(&delay), "reset delay {delay}");
    }
}
