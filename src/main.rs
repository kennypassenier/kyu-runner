//! kyu-runner: a stateless pump from the kyu hub to Home Assistant
//! webhooks. It long-polls configured topic subscriptions, forwards each
//! payload byte-for-byte to an HA webhook, and acks only on HA's 2xx —
//! so the hub's retry → dead-letter machinery works for the HA delivery.
//!
//! Built on chassis since 0.2.0: the kit owns the command line, the
//! configuration layers, logging, `/healthz`, `/metrics`, the graceful
//! shutdown and self-update. This file assembles the pump on top of it;
//! the pump itself starts in `kyu_runner::pump`.

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use chassis::{App, AppSpec};
use kyu_runner::config::Config;
use kyu_runner::health::{HealthState, RouteSubsystem};
use kyu_runner::pump::spawn_pump;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// The hub app token. Not a chassis knob on purpose: `KYU_RUNNER_TOKEN`
/// is the kit's dashboard login token, a different secret, so the hub
/// token moved to its own name with the migration (see CHANGELOG 0.2.0).
const HUB_TOKEN_ENV: &str = "KYU_RUNNER_HUB_TOKEN";

/// The hub itself sits on 8080 next to this pump (CT 109); the pump
/// answers /healthz and /metrics on its own port.
const DEFAULT_LISTEN: &str = "127.0.0.1:8082";

/// The route tasks the pump started, drained at shutdown.
type Tasks = Arc<Mutex<Vec<JoinHandle<()>>>>;

#[tokio::main]
async fn main() -> ExitCode {
    let spec = AppSpec {
        name: "kyu-runner",
        version: env!("CARGO_PKG_VERSION"),
        default_listen: DEFAULT_LISTEN,
        repository: Some("kennypassenier/kyu-runner"),
        ..Default::default()
    };
    // No public routes of its own: /healthz and /metrics come from the kit.
    let mut app = match App::from_env_and_args(spec, Router::new()) {
        Ok(app) => app,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    // Only a real start and `--check` need the pump's own config (AR20);
    // `--version`, `gen-secret`, `--healthcheck`, `--print-config`, `update`
    // and `rekey` are the kit's alone and must work without the file. Which
    // invocations those are is the kit's to know, not this file's.
    if !app.needs_project_config() {
        return app.run().await;
    }
    let config = match load_config(&app) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let routes = config.routes.len();
    app.on_check(move || {
        println!("config OK: {routes} route(s)");
        Ok(())
    });
    let health = register_health(&mut app, &config);
    install_lifecycle(&mut app, Arc::new(config), health);
    app.run().await
}

/// The pump's own config lives in the same TOML file as the kit's knobs;
/// the kit hands over the half that is ours, with its own keys and
/// sections removed, and the pump validates that with
/// `deny_unknown_fields` intact. The error is the line to print.
fn load_config(app: &App) -> Result<Config, String> {
    let table = app.project_table().map_err(|error| error.to_string())?;
    Config::from_project_table(&table).map_err(|error| format!("kyu-runner: {error}"))
}

/// One health subsystem per route, plus the two counter series (each
/// its own scrape source, feat-metrics-1) appended to the kit's
/// `/metrics`.
fn register_health(app: &mut App, config: &Config) -> Arc<HealthState> {
    let health = Arc::new(HealthState::new(
        &app.spec.metric_prefix(),
        config.routes.iter().map(|route| route.name.clone()),
    ));
    for route in &config.routes {
        app.subsystem(RouteSubsystem::new(&route.name, Arc::clone(&health)));
    }
    app.metrics_source(health.counters().delivered());
    app.metrics_source(health.counters().nacked());
    health
}

/// The pump starts once the kit is listening (on_start) and stops in
/// the kit's shutdown window (on_flush): in-flight deliveries finish,
/// unacked ones redeliver from the hub (AR9/W1).
fn install_lifecycle(app: &mut App, config: Arc<Config>, health: Arc<HealthState>) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let tasks: Tasks = Arc::new(Mutex::new(Vec::new()));
    let shutdown_grace = config.shutdown_grace();
    {
        let tasks = Arc::clone(&tasks);
        app.on_start(move || {
            // AR7: the hub token comes from the environment (systemd
            // EnvironmentFile), never from the config file in git.
            let token = std::env::var(HUB_TOKEN_ENV)
                .ok()
                .filter(|token| !token.is_empty());
            if token.is_none() {
                tracing::warn!(
                    "{HUB_TOKEN_ENV} is not set: every route will be denied by the hub (401). \
                     Mint an app token for \"kyu-runner\" on the hub's /apps page and set it in \
                     the unit's EnvironmentFile"
                );
            }
            match spawn_pump(&config, token, health, stop_rx) {
                Ok(spawned) => tasks.lock().expect("tasks lock").extend(spawned),
                Err(e) => {
                    tracing::error!(error = %e, "the pump could not start; /healthz stays degraded")
                }
            }
        });
    }
    app.on_flush(move || {
        // A send error only means every route loop has already ended,
        // which is the state this signal asks for.
        let _ = stop_tx.send(true);
        drain_tasks(&tasks, shutdown_grace);
        tracing::info!("kyu-runner stopped");
    });
}

/// Wait for every route task until the shared deadline, aborting any
/// task still running when it passes.
fn drain_tasks(tasks: &Tasks, grace: Duration) {
    let handles: Vec<_> = tasks.lock().expect("tasks lock").drain(..).collect();
    let deadline = Instant::now() + grace;
    tokio::runtime::Handle::current().block_on(async move {
        for mut task in handles {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if tokio::time::timeout(remaining, &mut task).await.is_err() {
                task.abort();
            }
        }
    });
}
