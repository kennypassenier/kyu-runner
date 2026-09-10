//! W4/AR10 + W6: what the pump reports about itself. The kit serves
//! `/healthz` and `/metrics`; this module keeps the per-route state and
//! hands the kit one `Subsystem` per route, and since chassis 2.0.0 it
//! counts into the kit's own `Counter` type instead of formatting
//! Prometheus text itself. Metric names are unchanged and are a contract
//! with the monitoring: `kyu_runner_delivered_total`,
//! `kyu_runner_nacked_total`. Never payloads, never the token.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chassis::{Counter, Subsystem, SubsystemStatus};

struct RouteSlot {
    state: &'static str,
}

pub struct HealthState {
    routes: Mutex<BTreeMap<String, RouteSlot>>,
    counters: RouteCounters,
}

impl HealthState {
    /// `prefix` is the kit's metric prefix (`AppSpec::metric_prefix()`).
    pub fn new(prefix: &str, names: impl Iterator<Item = String>) -> Self {
        let names: Vec<String> = names.collect();
        Self {
            routes: Mutex::new(
                names
                    .iter()
                    .map(|name| (name.clone(), RouteSlot { state: "starting" }))
                    .collect(),
            ),
            counters: RouteCounters::new(prefix, names.into_iter()),
        }
    }

    /// The two series to register with `App::metrics_source`.
    pub fn counters(&self) -> &RouteCounters {
        &self.counters
    }

    pub fn set(&self, route: &str, state: &'static str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.state = state;
        }
    }

    /// W6: a message reached the webhook (2xx) and was settled.
    pub fn count_delivered(&self, route: &str) {
        self.counters.delivered.inc(&[("route", route)]);
    }

    /// W6: a delivery was handed back to the hub (any nack).
    pub fn count_nacked(&self, route: &str) {
        self.counters.nacked.inc(&[("route", route)]);
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
}

/// The W6 counters, formatted by the kit (feat-metrics-1) instead of by
/// hand: label values are escaped and the HELP/TYPE/sample order is the
/// kit's problem, so a route name can no longer invalidate the scrape —
/// which would take the kit's own metrics down with it.
///
/// Both series are created at 0 for every configured route at startup. A
/// counter otherwise appears only once it is first incremented, and a
/// route that has delivered nothing would be missing from `/metrics`
/// rather than sitting at zero — a difference the monitoring reads as
/// "gone", not as "idle".
#[derive(Clone)]
pub struct RouteCounters {
    delivered: Counter,
    nacked: Counter,
}

impl RouteCounters {
    pub fn new(prefix: &str, routes: impl Iterator<Item = String>) -> Self {
        let counters = Self {
            delivered: Counter::new(
                prefix,
                "delivered_total",
                "Messages delivered to the webhook and settled.",
            ),
            nacked: Counter::new(
                prefix,
                "nacked_total",
                "Deliveries handed back to the hub (nacked).",
            ),
        };
        for route in routes {
            counters.delivered.add(&[("route", &route)], 0);
            counters.nacked.add(&[("route", &route)], 0);
        }
        counters
    }

    pub fn delivered(&self) -> Counter {
        self.delivered.clone()
    }

    pub fn nacked(&self) -> Counter {
        self.nacked.clone()
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
