//! kyu-runner: a stateless pump from the kyu hub to Home Assistant
//! webhooks. It long-polls configured topic subscriptions, forwards each
//! payload byte-for-byte to an HA webhook, and acks only on HA's 2xx —
//! so the hub's retry → dead-letter machinery works for the HA delivery.
//!
//! This file is a thin shell: CLI parsing, config load, runtime wiring.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use kyu_runner::config;
use kyu_runner::health::{self, HealthState};
use kyu_runner::hub::HubClient;
use kyu_runner::route::RouteRunner;
use kyu_runner::webhook::WebhookClient;
use tokio::sync::watch;

const USAGE: &str = "\
kyu-runner — stateless pump from the kyu hub to Home Assistant webhooks

Usage:
  kyu-runner [--config <path>] [--check-config]
  kyu-runner --version | --help

Options:
  --config <path>   Config file (default: /etc/kyu-runner/config.toml)
  --check-config    Validate the config and exit; makes no network calls

Environment:
  KYU_RUNNER_TOKEN       App token for the hub (mint one on its /apps page)
  KYU_RUNNER_LOG         Log filter (default: info)
  KYU_RUNNER_LOG_FORMAT  \"json\" for one JSON object per line (Loki)
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
                    return Err("--config needs a path. Remedy: kyu-runner --config \
                         /etc/kyu-runner/config.toml"
                        .into());
                }
            },
            "--check-config" => args.check_only = true,
            "--version" | "-V" => {
                println!("kyu-runner {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            other => {
                return Err(format!(
                    "unknown argument {other:?}. Remedy: see kyu-runner --help."
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
            eprintln!("kyu-runner: {message}");
            return ExitCode::from(2);
        }
    };

    let config = match config::load(&args.config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("kyu-runner: {error}");
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
    let token = std::env::var("KYU_RUNNER_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "kyu-runner: cannot start the runtime: {error}. Remedy: this is an OS-level failure (threads/fds); check the machine."
            );
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run(config, token)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kyu-runner: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_env("KYU_RUNNER_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    if std::env::var("KYU_RUNNER_LOG_FORMAT").is_ok_and(|value| value == "json") {
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
            config.tuning.settle_timeout(),
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
        tokio::spawn(health::serve(
            listener,
            Arc::clone(&health),
            config.tuning.healthz_max_connections,
            config.tuning.healthz_timeout(),
        ));
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
        "kyu-runner started"
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
    tracing::info!("kyu-runner stopped");
    Ok(())
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

async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
