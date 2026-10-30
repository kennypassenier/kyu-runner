//! W3: pushing a route's `[routes.policy]` block to the hub once the
//! subscription exists.

use tracing::{debug, info, warn};

use super::{LoopState, RouteRunner};

/// Longest slice of the hub's policy answer that reaches the log.
const POLICY_LOG_CHARS: usize = 600;

impl RouteRunner {
    /// W3: the poll that just succeeded proved the subscription exists,
    /// so the PUT can land now. Failures retry on later iterations —
    /// warn once, then quiet (AR6). The very first claim of a fresh
    /// subscription still runs under the hub-default lease; a budget
    /// computed from a larger configured lease can then outlive that
    /// one claim under sustained failure, which at worst costs one
    /// legal duplicate (at-least-once).
    pub(super) async fn apply_policy(&self, name: &str, state: &mut LoopState) {
        if !state.policy_pending {
            return;
        }
        let Some(policy_json) = &self.policy_json else {
            state.policy_pending = false;
            return;
        };
        let result = self
            .hub
            .put_policy(&self.route.topic, &self.route.subscription, policy_json)
            .await;
        match result {
            Ok(in_force) => {
                // Security F4: hub-controlled text, sanitized for the log.
                info!(
                    route = %name,
                    in_force = %crate::hub::printable(in_force.trim(), POLICY_LOG_CHARS),
                    "policy in force (W3) — the hub's answer is authoritative; note that a \
                     policy write replaces every field, so dashboard tweaks are reverted here"
                );
                state.policy_pending = false;
            }
            Err(error) if !state.policy_warned => {
                warn!(
                    route = %name, %error,
                    "policy PUT failed — retrying on later polls; until it lands the hub's \
                     current policy governs this subscription (W3)"
                );
                state.policy_warned = true;
            }
            Err(error) => debug!(route = %name, %error, "policy PUT still failing"),
        }
    }
}
