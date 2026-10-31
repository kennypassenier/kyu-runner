//! The runner under test as a child process handle.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::hub::Hub;

/// Passed as `KYU_RUNNER_TOKEN` to start the runner with no hub token at
/// all — the K9 case where the hub's door is shut in its face.
pub const NO_HUB_TOKEN: &str = "\u{0}none";

/// The kit's shutdown bound wraps the pump's own grace (AR9): wide
/// enough for the slowest delivery any test configures.
const SHUTDOWN_TIMEOUT_MS: &str = "20000";

/// Pause between two exit checks in `wait_exit`.
const EXIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct Runner {
    pub child: Child,
    pub log_path: PathBuf,
    _config: tempfile::NamedTempFile,
    _state: tempfile::TempDir,
}

impl Runner {
    pub fn start(hub: &Hub, config_text: &str) -> Runner {
        Self::start_with_env(hub, config_text, &[])
    }

    /// Start the runner against `hub`. Since kyu 3.0.0 every caller of the
    /// three verbs needs a bearer, so the hub's client token is passed by
    /// default; a test that wants the runner denied overrides
    /// `KYU_RUNNER_TOKEN` in `env` with a wrong one, or passes
    /// [`NO_HUB_TOKEN`] to send none at all.
    pub fn start_with_env(hub: &Hub, config_text: &str, env: &[(&str, &str)]) -> Runner {
        let (listen, config_text) = split_listen(config_text);
        let mut config = tempfile::NamedTempFile::new().expect("config file");
        config
            .write_all(config_text.as_bytes())
            .expect("write config");
        let state = tempfile::tempdir().expect("state dir");
        let log = tempfile::NamedTempFile::new().expect("log file");
        let (log_file, log_path) = log.keep().expect("keep log");
        let mut command = Command::new(runner_binary());
        command
            .arg("--config")
            .arg(config.path())
            .arg("--state-dir")
            .arg(state.path())
            .arg("--listen")
            .arg(&listen)
            .args(["--shutdown-timeout-ms", SHUTDOWN_TIMEOUT_MS])
            // Crate-scoped debug: hyper/reqwest connect spam would break
            // the K7 log-volume bound and drown the assertions.
            .env("KYU_RUNNER_LOG", "info,kyu_runner=debug")
            .stdout(Stdio::null())
            .stderr(log_file);
        apply_env(&mut command, hub, env);
        let child = command.spawn().expect("spawn runner");
        Runner {
            child,
            log_path,
            _config: config,
            _state: state,
        }
    }

    pub fn log(&self) -> String {
        // An unreadable log reads as empty: callers poll it until a
        // line appears, and a missing file just means "not yet".
        std::fs::read_to_string(&self.log_path).unwrap_or_default()
    }

    pub fn kill_hard(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    pub fn sigterm(&self) {
        self.signal("-TERM");
    }

    pub fn signal(&self, signal: &str) {
        let _ = Command::new("kill")
            .args([signal, &self.child.id().to_string()])
            .output();
    }

    pub fn wait_exit(&mut self, within: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            std::thread::sleep(EXIT_POLL_INTERVAL);
        }
        None
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.kill_hard();
        let _ = std::fs::remove_file(&self.log_path);
    }
}

/// Since the chassis migration the observation socket is the kit's and
/// always listens: a test that used to opt in with
/// `healthz_listen = "..."` in the config now gets that address as
/// `--listen`; every other test listens on an ephemeral port. Returns
/// the address and the config with that key removed.
fn split_listen(config_text: &str) -> (String, String) {
    let mut listen = "127.0.0.1:0".to_string();
    let mut kept = Vec::new();
    for line in config_text.lines() {
        if let Some(rest) = line.trim().strip_prefix("healthz_listen") {
            if let Some(addr) = rest.split('"').nth(1) {
                listen = addr.to_string();
            }
            continue;
        }
        kept.push(line);
    }
    (listen, kept.join("\n"))
}

/// KYU_RUNNER_BIN lets a person run this suite against the musl
/// artifact (e.g. dist/kyu-runner) to test the shipped binary (T8/M1).
/// Nothing sets it automatically: the release workflow that did was
/// replaced at 0.2.0 (2026-09-05).
fn runner_binary() -> String {
    std::env::var("KYU_RUNNER_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_kyu-runner").into())
}

/// The hub's door is always on since 3.0.0, so the runner gets a
/// working token unless the test says otherwise.
fn apply_env(command: &mut Command, hub: &Hub, env: &[(&str, &str)]) {
    command.env("KYU_RUNNER_HUB_TOKEN", hub.client_token());
    for (name, value) in env {
        // The hub token has its own name since 0.2.0 (KYU_RUNNER_TOKEN
        // is the kit's dashboard login token); tests keep the old name.
        if *name == "KYU_RUNNER_TOKEN" {
            if *value == NO_HUB_TOKEN {
                command.env_remove("KYU_RUNNER_HUB_TOKEN");
            } else {
                command.env("KYU_RUNNER_HUB_TOKEN", value);
            }
        } else {
            command.env(name, value);
        }
    }
}
