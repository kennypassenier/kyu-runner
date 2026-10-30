//! Fake HA webhook server: records every request and answers by mode.

use std::net::TcpListener as StdTcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::fake_ha_wire::handle;
use super::ports::free_port;

/// Pause between two non-blocking `accept` attempts.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How often and how long `start_on` retries a port whose previous
/// owner is still releasing it.
const BIND_ATTEMPTS: u32 = 50;
const BIND_RETRY_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub enum HaMode {
    Ok200,
    Fail(u16),
    Redirect,
    /// Answer 200 after a delay — for kill/SIGTERM drills.
    SlowOk(Duration),
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub method: String,
    pub path: String,
    pub content_type: Option<String>,
    pub headers: Vec<(String, String)>,
    /// Raw bytes: K2's byte-for-byte promise includes payloads that
    /// are not UTF-8 (gap audit #4).
    pub body: Vec<u8>,
}

impl Hit {
    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

pub struct FakeHa {
    pub port: u16,
    pub hits: Arc<Mutex<Vec<Hit>>>,
    mode: Arc<Mutex<HaMode>>,
    live: Arc<Mutex<bool>>,
}

impl FakeHa {
    pub fn start() -> FakeHa {
        // Bind the reserved port immediately: closing a probe listener
        // and rebinding later is the window that let two suites collide.
        let port = free_port();
        Self::start_on(port)
    }

    /// Binding a fixed port lets a test simulate "HA back after an
    /// outage" (AR15): drop one FakeHa, start another on the same port.
    /// The predecessor's socket may still be releasing, so retry
    /// briefly instead of failing the test over a millisecond.
    pub fn start_on(port: u16) -> FakeHa {
        let listener = bind_with_retry(port);
        let hits = Arc::new(Mutex::new(Vec::new()));
        let mode = Arc::new(Mutex::new(HaMode::Ok200));
        let live = Arc::new(Mutex::new(true));
        spawn_accept_loop(
            listener,
            Arc::clone(&hits),
            Arc::clone(&mode),
            Arc::clone(&live),
        );
        FakeHa {
            port,
            hits,
            mode,
            live,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    pub fn set_mode(&self, mode: HaMode) {
        *self.mode.lock().unwrap() = mode;
    }

    pub fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }

    /// Take the server down; the port stays reserved for a successor.
    pub fn shut_down(self) -> u16 {
        *self.live.lock().unwrap() = false;
        self.port
    }
}

fn bind_with_retry(port: u16) -> StdTcpListener {
    let mut attempt = 0;
    let listener = loop {
        match StdTcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => break listener,
            Err(error) => {
                attempt += 1;
                assert!(
                    attempt < BIND_ATTEMPTS,
                    "fake HA could not bind {port}: {error}"
                );
                std::thread::sleep(BIND_RETRY_INTERVAL);
            }
        }
    };
    listener.set_nonblocking(true).expect("nonblocking");
    listener
}

/// Accept connections until `live` turns false; each connection is
/// served on its own thread with the mode that was current on accept.
fn spawn_accept_loop(
    listener: StdTcpListener,
    hits: Arc<Mutex<Vec<Hit>>>,
    mode: Arc<Mutex<HaMode>>,
    live: Arc<Mutex<bool>>,
) {
    std::thread::spawn(move || {
        loop {
            if !*live.lock().unwrap() {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    let hits = Arc::clone(&hits);
                    let mode = mode.lock().unwrap().clone();
                    std::thread::spawn(move || handle(stream, hits, mode));
                }
                Err(ref error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL_INTERVAL);
                }
                // The listener is gone: nothing left to accept.
                Err(_) => return,
            }
        }
    });
}
