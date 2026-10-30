//! Starting the pump: build the hub and webhook clients, then run one
//! supervised loop per route (AR1). `main.rs` wires this into the kit's
//! lifecycle; keeping it in the library lets it be read and tested
//! apart from the process setup.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::health::HealthState;
use crate::hub::HubClient;
use crate::route::RouteRunner;
use crate::webhook::WebhookClient;

/// Build the clients and start one supervised loop per route.
pub fn spawn_pump(
    config: &Config,
    token: Option<String>,
    health: Arc<HealthState>,
    stop_rx: watch::Receiver<bool>,
) -> Result<Vec<JoinHandle<()>>, chassis::Error> {
    let (hub, webhook) = build_clients(config, token)?;
    let tasks = config
        .routes
        .iter()
        .map(|route| {
            let runner = RouteRunner {
                route: route.clone(),
                hub: Arc::clone(&hub),
                webhook: Arc::clone(&webhook),
                webhook_timeout: config.webhook_timeout(route),
                lease_budget: config.lease_budget(route),
                policy_json: Config::policy_json(route),
                health: Arc::clone(&health),
                hub_backoff: config.tuning.hub_backoff(),
                circuit_backoff: config.tuning.circuit_backoff(),
                circuit_probe_timeout: config.tuning.circuit_probe_timeout(),
                topic_unborn_poll: config.tuning.topic_unborn_poll(),
            };
            tokio::spawn(supervise(
                runner,
                stop_rx.clone(),
                config.tuning.route_respawn(),
            ))
        })
        .collect();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        hub = %config.hub_url,
        routes = config.routes.len(),
        "kyu-runner pump started"
    );
    Ok(tasks)
}

/// The two HTTP clients every route shares, each failure turned into a
/// kit error with a remedy.
fn build_clients(
    config: &Config,
    token: Option<String>,
) -> Result<(Arc<HubClient>, Arc<WebhookClient>), chassis::Error> {
    let hub = HubClient::new(
        &config.hub_url,
        token,
        config.poll_wait(),
        config.defaults.max_body_bytes,
        config.tuning.settle_timeout(),
    )
    .map_err(|e| {
        chassis::Error::config(
            format!("cannot build the hub client: {e}"),
            "check hub_url in the config file",
        )
    })?;
    let webhook = WebhookClient::new().map_err(|e| {
        chassis::Error::internal(
            format!("cannot build the webhook client: {e}"),
            "report this",
        )
    })?;
    Ok((Arc::new(hub), Arc::new(webhook)))
}

/// AR1: a route task never takes the process down — a panic inside the
/// loop is logged and the loop respawns after a pause. The unacked
/// claim it may have held redelivers on its own (K5).
async fn supervise(runner: RouteRunner, shutdown: watch::Receiver<bool>, respawn_after: Duration) {
    loop {
        let name = runner.route.name.clone();
        let handle = tokio::spawn(runner.clone().run(shutdown.clone()));
        match handle.await {
            Ok(()) => return,
            Err(error) => {
                tracing::error!(
                    route = %name, %error, respawn_after_ms = respawn_after.as_millis() as u64,
                    "route loop died unexpectedly — respawning (AR1); an unacked claim \
                     redelivers by itself (K5)"
                );
                runner.health.set(&name, "respawning");
                if *shutdown.borrow() {
                    return;
                }
                tokio::time::sleep(respawn_after).await;
            }
        }
    }
}
