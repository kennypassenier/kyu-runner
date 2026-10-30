//! How the scratch hub process is started: a kyu binary or a docker
//! container, plus the lookup of which binary that is.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The hub's admin token when a test does not pick one. Long enough for
/// the hub's own minimum (it refuses tokens under 16 characters) and
/// deliberately not a secret: it protects a throwaway hub on localhost.
pub(super) const ADMIN_TOKEN: &str = "suite-admin-token-lang-genoeg";

/// 64 hex chars: the hub requires KYU_SECRET_KEY next to the token; a
/// fixed test key is fine, nothing real is protected.
const SECRET_KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// The port the hub listens on inside its docker container.
const DOCKER_INTERNAL_PORT: u16 = 8080;

pub(super) enum HubProcess {
    Binary { child: Child },
    Docker { container: String },
}

impl HubProcess {
    /// Kill the process or remove the container; both are best effort
    /// because the hub may already be gone (a drill stops it first).
    pub(super) fn terminate(self) {
        match self {
            HubProcess::Binary { mut child } => {
                let _ = child.kill();
                let _ = child.wait();
            }
            HubProcess::Docker { container } => {
                let _ = Command::new("docker")
                    .args(["rm", "-f", &container])
                    .output();
            }
        }
    }
}

pub(super) fn hub_binary() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("KYU_BIN") {
        // KYU_BIN="" forces the docker path even when the default
        // binary exists — how the docker-only path is exercised.
        return (!path.is_empty()).then(|| PathBuf::from(path));
    }
    // The newest signed kyu release, verified and cached by the
    // workstation's kyu-latest (Kenny, 2026-10-04: tests run against the
    // latest kyu, never a pinned one; the pinned ghcr image was deleted
    // when kyu became a native service).
    // Asked once per test binary: every Hub::start shares the answer.
    static LATEST: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    let home = std::env::var("HOME").ok()?;
    Some(LATEST.get_or_init(|| latest_kyu(&home)).clone())
}

fn latest_kyu(home: &str) -> PathBuf {
    let helper = PathBuf::from(home).join("Projects/workstation/bin/kyu-latest");
    let out = Command::new(&helper)
        .output()
        .unwrap_or_else(|e| panic!("{}: {e}; set KYU_BIN to a kyu binary", helper.display()));
    assert!(
        out.status.success(),
        "kyu-latest failed: {}; set KYU_BIN to a kyu binary",
        String::from_utf8_lossy(&out.stderr)
    );
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
}

/// Start the hub binary; returns the process and the path of the file
/// holding its stderr.
pub(super) fn spawn_binary(binary: &Path, listen: &str, data: &Path) -> (HubProcess, PathBuf) {
    let log = tempfile::NamedTempFile::new().expect("hub log file");
    let (log_file, log_path) = log.keep().expect("keep hub log");
    // Since kyu 3.0.0 the dashboard is compiled in and the hub refuses
    // to start without both secrets, so they are no longer optional
    // the way the 2.x door was. A test that wants the runner denied
    // gives the runner a wrong token, not the hub a missing one.
    let child = Command::new(binary)
        .env("KYU_LISTEN", listen)
        // KYU_DATA_DIR is the 2.x name and warns since 3.0.0.
        .env("KYU_STATE_DIR", data)
        .env("KYU_LOG", "warn")
        .env("KYU_TOKEN", ADMIN_TOKEN)
        .env("KYU_SECRET_KEY", SECRET_KEY)
        .stdout(Stdio::null())
        .stderr(log_file)
        .spawn()
        .expect("spawn kyu binary (set KYU_BIN or build ~/Projects/kyu)");
    (HubProcess::Binary { child }, log_path)
}

/// Start the hub in docker. Only on request (KYU_BIN="" plus
/// KYU_IMAGE): no kyu image is published any more, so there is no
/// default to fall back on.
pub(super) fn spawn_docker(port: u16) -> HubProcess {
    let image = std::env::var("KYU_IMAGE").expect(
        "KYU_BIN is empty: set KYU_IMAGE to a kyu image, or unset KYU_BIN to use the latest kyu",
    );
    let args = [
        "run".to_string(),
        "-d".into(),
        "-p".into(),
        format!("127.0.0.1:{port}:{DOCKER_INTERNAL_PORT}"),
        "-e".into(),
        format!("KYU_LISTEN=0.0.0.0:{DOCKER_INTERNAL_PORT}"),
        "-e".into(),
        "KYU_LOG=warn".into(),
        "-e".into(),
        format!("KYU_TOKEN={ADMIN_TOKEN}"),
        "-e".into(),
        format!("KYU_SECRET_KEY={SECRET_KEY}"),
        image,
    ];
    let output = Command::new("docker")
        .args(&args)
        .output()
        .expect("docker run (no KYU_BIN and no docker — one is required)");
    assert!(
        output.status.success(),
        "docker run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    HubProcess::Docker {
        container: String::from_utf8_lossy(&output.stdout).trim().to_string(),
    }
}
