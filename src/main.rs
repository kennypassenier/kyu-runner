//! hub-bridge: a stateless pump from the mailbox hub to Home Assistant
//! webhooks. It long-polls configured topic subscriptions, forwards each
//! payload byte-for-byte to an HA webhook, and acks only on HA's 2xx —
//! so the hub's retry → dead-letter machinery works for the HA delivery.
//!
//! This file is a thin shell: CLI parsing, config load, runtime wiring.

use std::path::PathBuf;
use std::process::ExitCode;

use hub_bridge::config;

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

    // Loud, not silent (standing rule 12): the pump arrives with L2.
    eprintln!(
        "hub-bridge: run mode is not built yet (milestone L2); only --check-config works today."
    );
    ExitCode::FAILURE
}
