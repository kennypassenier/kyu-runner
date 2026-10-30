//! E2E support: a real scratch hub (standing rule 9 — the real thing,
//! never a mock of the hub), a fake HA webhook server (HA cannot be
//! real in a test — what the fake cannot express is recorded in
//! TEST_PLAN.md), and a runner process handle.
//!
//! Layout: `ports` (cross-process port claims), `hub_launch` + `hub`
//! (the scratch hub), `hub_api` (verbs against it), `fake_ha`, `runner`.
//! Every item is re-exported here so tests keep writing `support::X`.

#![allow(dead_code, unused_imports)]

use std::time::{Duration, Instant};

mod fake_ha;
mod fake_ha_wire;
mod hub;
mod hub_api;
mod hub_launch;
mod ports;
mod runner;

pub use fake_ha::*;
pub use hub::Hub;
pub use hub_api::*;
pub use ports::{free_port, reserve};
pub use runner::{NO_HUB_TOKEN, Runner};

/// How long `wait_first_poll` waits for the runner's first poll.
const FIRST_POLL_TIMEOUT: Duration = Duration::from_secs(15);

/// Pause between two checks in `wait_until`.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

pub fn route_config(hub: &Hub, name: &str, topic: &str, webhook_url: &str) -> String {
    format!(
        "hub_url = \"{}\"\n\n[[routes]]\nname = \"{name}\"\ntopic = \"{topic}\"\n\
         subscription = \"ha-runner\"\nwebhook_url = \"{webhook_url}\"\n",
        hub.base()
    )
}

/// Wait until the runner has polled a not-yet-born topic once (the
/// AR16 log line). Publishing before the runner's first poll would
/// create the topic under the runner's feet and the subscription would
/// start "from now" — the documented boundary the runbook orders
/// around ("route first, then producers").
pub async fn wait_first_poll(runner: &Runner) {
    let log = || runner.log();
    wait_until(
        "the runner's first poll (topic not born line)",
        FIRST_POLL_TIMEOUT,
        || log().contains("topic not born"),
    )
    .await;
}

pub async fn wait_until<F: FnMut() -> bool>(what: &str, within: Duration, mut check: F) {
    let deadline = Instant::now() + within;
    loop {
        if check() {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(WAIT_POLL_INTERVAL).await;
    }
}
