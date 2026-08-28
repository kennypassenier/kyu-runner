//! hub-bridge: a stateless pump from the mailbox hub to Home Assistant
//! webhooks. It long-polls configured topic subscriptions, forwards each
//! payload byte-for-byte to an HA webhook, and acks only on HA's 2xx —
//! so the hub's retry → dead-letter machinery works for the HA delivery.
//!
//! This file is a thin shell: CLI parsing, config load, runtime wiring.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use hub_bridge::config;
use hub_bridge::health::{self, HealthState};
use hub_bridge::hub::HubClient;
use hub_bridge::route::RouteRunner;
use hub_bridge::webhook::WebhookClient;
use tokio::sync::watch;

const USAGE: &str = "\
hub-bridge — stateless pump from the mailbox hub to Home Assistant webhooks

Usage:
  hub-bridge [--config <path>] [--check-config]
  hub-bridge --version | --help

Options:
  --config <path>   Config file (default: /etc/hub-bridge/config.toml)
  --check-config    Validate the config and exit; makes no network calls

Environment:
  HUB_BRIDGE_TOKEN       App token for the hub (mint one on its /apps page)
  HUB_BRIDGE_LOG         Log filter (default: info)
  HUB_BRIDGE_LOG_FORMAT  \"json\" for one JSON object per line (Loki)
";

struct Args {
    config_path: PathBuf,
    check_only: bool,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        config_path: PathBuf::from(config::DEFAULT_CONFIG_PATH),
        check_only: false,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--config" => match raw.next() {
                Some(path) => args.config_path = PathBuf::from(path),
                None => {
                    return Err("--config needs a path. Remedy: hub-bridge --config \
                         /etc/hub-bridge/config.toml"
                        .into());
                }
            },
            "--check-config" => args.check_only = true,
            "--version" | "-V" => {
                println!("hub-bridge {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            other => {
                return Err(format!(
                    "unknown argument {other:?}. Remedy: see hub-bridge --help."
                ));
            }
        }
    }
    Ok(Some(args))
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("hub-bridge: {message}");
            return ExitCode::from(2);
        }
    };

    let config = match config::load(&args.config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("hub-bridge: {error}");
            return ExitCode::FAILURE;
        }
    };

    if args.check_only {
        println!("config OK: {} route(s)", config.routes.len());
        return ExitCode::SUCCESS;
    }

    init_tracing();
    // AR7: the token comes from the environment (systemd
    // EnvironmentFile), never from the config file that lives in git.
    let token = std::env::var("HUB_BRIDGE_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "hub-bridge: cannot start the runtime: {error}. Remedy: this is an OS-level failure (threads/fds); check the machine."
            );
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(config, token)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hub-bridge: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_env("HUB_BRIDGE_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if std::env::var("HUB_BRIDGE_LOG_FORMAT").is_ok_and(|value| value == "json") {
        builder.json().init();
    } else {
        builder.init();
    }
}

async fn run(config: config::Config, token: Option<String>) -> anyhow::Result<()> {
    use anyhow::Context;

    let hub = Arc::new(
        HubClient::new(
            &config.hub_url,
            token,
            config.poll_wait(),
            config.defaults.max_body_bytes,
        )
        .context("cannot build the hub client")?,
    );
    let webhook = Arc::new(WebhookClient::new().context("cannot build the webhook client")?);

    let health = Arc::new(HealthState::new(
        config.routes.iter().map(|route| route.name.clone()),
    ));
    if let Some(listen) = &config.healthz_listen {
        // Fail-closed (AR10): a health endpoint that silently failed to
        // bind would report exactly nothing, which is the failure mode
        // W4 exists to prevent.
        let listener = tokio::net::TcpListener::bind(listen)
            .await
            .with_context(|| {
                format!(
                    "cannot open the health endpoint on {listen}. Remedy: free the port or change \
                 healthz_listen in the config"
                )
            })?;
        tokio::spawn(health::serve(listener, Arc::clone(&health)));
    }

    let (stop_tx, stop_rx) = watch::channel(false);
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
        };
        tasks.push(tokio::spawn(supervise(runner, stop_rx.clone())));
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        hub = %config.hub_url,
        routes = config.routes.len(),
        "hub-bridge started"
    );

    wait_for_signal().await;
    tracing::info!("shutdown signal — letting in-flight deliveries finish (AR9/W1)");
    let _ = stop_tx.send(true);
    tokio::spawn(async {
        wait_for_signal().await;
        tracing::warn!("second signal — exiting immediately; unacked messages redeliver (K5)");
        std::process::exit(130);
    });

    let deadline = std::time::Instant::now() + config.shutdown_grace();
    for task in tasks {
        let mut task = task;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if tokio::time::timeout(remaining, &mut task).await.is_err() {
            task.abort();
        }
    }
    tracing::info!("hub-bridge stopped");
    Ok(())
}

/// AR1: a route task never takes the process down — a panic inside the
/// loop is logged and the loop respawns after a pause. The unacked
/// claim it may have held redelivers on its own (K5).
async fn supervise(runner: RouteRunner, shutdown: watch::Receiver<bool>) {
    loop {
        let name = runner.route.name.clone();
        let handle = tokio::spawn(runner.clone().run(shutdown.clone()));
        match handle.await {
            Ok(()) => return,
            Err(error) => {
                tracing::error!(
                    route = %name, %error,
                    "route loop died unexpectedly — respawning in 5 s (AR1); an unacked claim \
                     redelivers by itself (K5)"
                );
                runner.health.set(&name, "respawning");
                if *shutdown.borrow() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
