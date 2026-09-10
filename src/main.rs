//! kyu-runner: a stateless pump from the kyu hub to Home Assistant
//! webhooks. It long-polls configured topic subscriptions, forwards each
//! payload byte-for-byte to an HA webhook, and acks only on HA's 2xx —
//! so the hub's retry → dead-letter machinery works for the HA delivery.
//!
//! Built on chassis since 0.2.0: the kit owns the command line, the
//! configuration layers, logging, `/healthz`, `/metrics`, the graceful
//! shutdown and self-update. This file assembles the pump on top of it.

use std::sync::Arc;

use axum::Router;
use chassis::{App, AppSpec};
use kyu_runner::config;
use kyu_runner::health::{HealthState, RouteSubsystem};
use kyu_runner::hub::HubClient;
use kyu_runner::route::RouteRunner;
use kyu_runner::webhook::WebhookClient;
use tokio::sync::watch;

/// The hub app token. Not a chassis knob on purpose: `KYU_RUNNER_TOKEN`
/// is the kit's dashboard login token, a different secret, so the hub
/// token moved to its own name with the migration (see CHANGELOG 0.2.0).
const HUB_TOKEN_ENV: &str = "KYU_RUNNER_HUB_TOKEN";

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let spec = AppSpec {
        name: "kyu-runner",
        version: env!("CARGO_PKG_VERSION"),
        // The hub itself sits on 8080 next to this pump (CT 109); the
        // pump answers /healthz and /metrics on its own port.
        default_listen: "127.0.0.1:8082",
        repository: Some("kennypassenier/kyu-runner"),
        ..Default::default()
    };
    // No public routes of its own: /healthz and /metrics come from the kit.
    let mut app = match App::from_env_and_args(spec, Router::new()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("{e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // Only a real start and `--check` need the pump's own config (AR20);
    // `--version`, `gen-secret`, `--healthcheck`, `--print-config`, `update`
    // and `rekey` are the kit's alone and must work without the file. Which
    // invocations those are is the kit's to know, not this file's.
    if !app.needs_project_config() {
        return app.run().await;
    }
    // The pump's own config lives in the same TOML file as the kit's knobs;
    // the kit hands over the half that is ours, with its own keys and
    // sections removed, and the pump validates that with
    // `deny_unknown_fields` intact.
    let table = match app.project_table() {
        Ok(table) => table,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let config = match config::Config::from_project_table(&table) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("kyu-runner: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let routes = config.routes.len();
    app.on_check(move || {
        println!("config OK: {routes} route(s)");
        Ok(())
    });

    let health = Arc::new(HealthState::new(
        &app.spec.metric_prefix(),
        config.routes.iter().map(|route| route.name.clone()),
    ));
    for route in &config.routes {
        app.subsystem(RouteSubsystem::new(&route.name, Arc::clone(&health)));
    }
    // Two series, each its own scrape source (feat-metrics-1); the kit
    // renders them and appends them to its own `/metrics`.
    app.metrics_source(health.counters().delivered());
    app.metrics_source(health.counters().nacked());

    // The pump starts once the kit is listening (on_start) and stops in
    // the kit's shutdown window (on_flush): in-flight deliveries finish,
    // unacked ones redeliver from the hub (AR9/W1).
    let (stop_tx, stop_rx) = watch::channel(false);
    let tasks: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let config = Arc::new(config);
    {
        let config = Arc::clone(&config);
        let health = Arc::clone(&health);
        let tasks = Arc::clone(&tasks);
        app.on_start(move || {
            // AR7: the hub token comes from the environment (systemd
            // EnvironmentFile), never from the config file in git.
            let token = std::env::var(HUB_TOKEN_ENV)
                .ok()
                .filter(|token| !token.is_empty());
            match spawn_pump(&config, token, health, stop_rx) {
                Ok(spawned) => tasks.lock().expect("tasks lock").extend(spawned),
                Err(e) => {
                    tracing::error!(error = %e, "the pump could not start; /healthz stays degraded")
                }
            }
        });
    }
    let shutdown_grace = config.shutdown_grace();
    app.on_flush(move || {
        let _ = stop_tx.send(true);
        let handles: Vec<_> = tasks.lock().expect("tasks lock").drain(..).collect();
        let deadline = std::time::Instant::now() + shutdown_grace;
        tokio::runtime::Handle::current().block_on(async move {
            for mut task in handles {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if tokio::time::timeout(remaining, &mut task).await.is_err() {
                    task.abort();
                }
            }
        });
        tracing::info!("kyu-runner stopped");
    });
    app.run().await
}

/// Build the clients and start one supervised loop per route.
fn spawn_pump(
    config: &config::Config,
    token: Option<String>,
    health: Arc<HealthState>,
    stop_rx: watch::Receiver<bool>,
) -> Result<Vec<tokio::task::JoinHandle<()>>, chassis::Error> {
    let hub = Arc::new(
        HubClient::new(
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
        })?,
    );
    let webhook = Arc::new(WebhookClient::new().map_err(|e| {
        chassis::Error::internal(
            format!("cannot build the webhook client: {e}"),
            "report this",
        )
    })?);
    let mut tasks = Vec::new();
    for route in &config.routes {
        let runner = RouteRunner {
            route: route.clone(),
            hub: Arc::clone(&hub),
            webhook: Arc::clone(&webhook),
            webhook_timeout: config.webhook_timeout(route),
            lease_budget: config.lease_budget(route),
            policy_json: config::Config::policy_json(route),
            health: Arc::clone(&health),
            hub_backoff: config.tuning.hub_backoff(),
            circuit_backoff: config.tuning.circuit_backoff(),
            circuit_probe_timeout: config.tuning.circuit_probe_timeout(),
            topic_unborn_poll: config.tuning.topic_unborn_poll(),
        };
        tasks.push(tokio::spawn(supervise(
            runner,
            stop_rx.clone(),
            config.tuning.route_respawn(),
        )));
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        hub = %config.hub_url,
        routes = config.routes.len(),
        "kyu-runner pump started"
    );
    Ok(tasks)
}

/// AR1: a route task never takes the process down — a panic inside the
/// loop is logged and the loop respawns after a pause. The unacked
/// claim it may have held redelivers on its own (K5).
async fn supervise(
    runner: RouteRunner,
    shutdown: watch::Receiver<bool>,
    respawn_after: std::time::Duration,
) {
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
