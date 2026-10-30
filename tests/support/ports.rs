//! Cross-process port reservation for the scratch hub and the fake HA.

use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::time::Duration;

/// A port claim older than this belongs to a finished test run and is
/// swept; without the sweep the claim directory would slowly fill with
/// every ephemeral port the machine ever handed out.
const STALE_CLAIM_AFTER: Duration = Duration::from_secs(3600);

/// How many ephemeral ports are probed before giving up on finding one
/// that no other suite process has claimed yet.
const RESERVE_ATTEMPTS: usize = 200;

/// The shared claim directory, created and swept once per process.
fn claim_dir() -> &'static PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("kyu-runner-test-ports");
        if let Err(error) = std::fs::create_dir_all(&dir) {
            panic!("create port claim dir {}: {error}", dir.display());
        }
        sweep_stale_claims(&dir);
        dir
    })
}

/// Remove claims left behind by earlier runs. A claim that cannot be
/// inspected or removed is left in place: a stale file only costs one
/// unusable port number, never a wrong claim.
fn sweep_stale_claims(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|when| when.elapsed().unwrap_or_default() > STALE_CLAIM_AFTER)
            .unwrap_or(false);
        if stale {
            // Another process may have swept the same file first.
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Reserve a port ACROSS PROCESSES.
///
/// `cargo test --all` runs every suite as its own process, so an
/// in-process set is not enough: two suites probing `:0` at the same
/// moment can be handed the same port, and whichever binds second dies
/// with "Address already in use". That is exactly the flake that broke
/// two commit gates on 2026-08-30. The claim therefore lives in a
/// shared directory, where `create_new` is the atomic winner-takes-it
/// primitive every process can see.
pub fn reserve(port: u16) -> bool {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(claim_dir().join(port.to_string()))
        .is_ok()
}

pub fn free_port() -> u16 {
    for _ in 0..RESERVE_ATTEMPTS {
        let port = StdTcpListener::bind("127.0.0.1:0")
            .expect("bind ephemeral")
            .local_addr()
            .expect("local addr")
            .port();
        if reserve(port) {
            return port;
        }
    }
    panic!("no free port could be reserved after {RESERVE_ATTEMPTS} attempts");
}
