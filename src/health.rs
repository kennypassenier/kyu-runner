//! W4/AR10 + W6: what the pump reports about itself. Since the chassis
//! migration the kit serves `/healthz` and `/metrics`; this module keeps
//! the per-route state and counters and hands them to the kit as one
//! `Subsystem` per route and one `ScrapeSource` (metric names unchanged:
//! `kyu_runner_delivered_total`, `kyu_runner_nacked_total`). Never
//! payloads, never the token. Route names are K8-restricted to
//! `[A-Za-z0-9._-]`, so the metric labels cannot need escaping.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chassis::{ScrapeSource, Subsystem, SubsystemStatus};

struct RouteSlot {
    state: &'static str,
    delivered: u64,
    nacked: u64,
}

pub struct HealthState {
    routes: Mutex<BTreeMap<String, RouteSlot>>,
}

impl HealthState {
    pub fn new(names: impl Iterator<Item = String>) -> Self {
        Self {
            routes: Mutex::new(
                names
                    .map(|name| {
                        (
                            name,
                            RouteSlot {
                                state: "starting",
                                delivered: 0,
                                nacked: 0,
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }

    pub fn set(&self, route: &str, state: &'static str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.state = state;
        }
    }

    /// W6: a message reached the webhook (2xx) and was settled.
    pub fn count_delivered(&self, route: &str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.delivered += 1;
        }
    }

    /// W6: a delivery was handed back to the hub (any nack).
    pub fn count_nacked(&self, route: &str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.nacked += 1;
        }
    }

    /// The loop state the pump last reported for `route`.
    pub fn state_of(&self, route: &str) -> &'static str {
        self.routes
            .lock()
            .expect("health lock")
            .get(route)
            .map(|slot| slot.state)
            .unwrap_or("unknown")
    }

    pub fn render_health(&self) -> String {
        let routes = self.routes.lock().expect("health lock");
        let items: Vec<String> = routes
            .iter()
            .map(|(name, slot)| format!("{{\"name\":\"{name}\",\"state\":\"{}\"}}", slot.state))
            .collect();
        format!("{{\"status\":\"ok\",\"routes\":[{}]}}", items.join(","))
    }

    /// W6: Prometheus text format, counters only.
    pub fn render_metrics(&self) -> String {
        let routes = self.routes.lock().expect("health lock");
        let mut out = String::from(
            "# HELP kyu_runner_delivered_total Messages delivered to the webhook and settled.\n\
             # TYPE kyu_runner_delivered_total counter\n",
        );
        for (name, slot) in routes.iter() {
            out.push_str(&format!(
                "kyu_runner_delivered_total{{route=\"{name}\"}} {}\n",
                slot.delivered
            ));
        }
        out.push_str(
            "# HELP kyu_runner_nacked_total Deliveries handed back to the hub (nacked).\n\
             # TYPE kyu_runner_nacked_total counter\n",
        );
        for (name, slot) in routes.iter() {
            out.push_str(&format!(
                "kyu_runner_nacked_total{{route=\"{name}\"}} {}\n",
                slot.nacked
            ));
        }
        out
    }
}

/// One `/healthz` subsystem per route (K6 of the kit): the detail is the
/// loop state the pump reports; the three states that mean "not
/// delivering because of something outside this process" count as
/// failing, so the kit answers 503 while they last.
pub struct RouteSubsystem {
    name: String,
    health: Arc<HealthState>,
}

impl RouteSubsystem {
    pub fn new(name: &str, health: Arc<HealthState>) -> Self {
        Self {
            name: name.to_string(),
            health,
        }
    }
}

impl Subsystem for RouteSubsystem {
    fn name(&self) -> &str {
        &self.name
    }
    fn check(&self) -> SubsystemStatus {
        let state = self.health.state_of(&self.name);
        match state {
            "hub-down" | "auth-denied" | "circuit-open" => SubsystemStatus::failing(state),
            other => SubsystemStatus::ok(other),
        }
    }
}

/// The W6 counters, appended verbatim to the kit's `/metrics`.
pub struct RouteCounters(pub Arc<HealthState>);

impl ScrapeSource for RouteCounters {
    fn scrape(&self) -> String {
        self.0.render_metrics()
    }
}
