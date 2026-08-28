//! W4/AR10: the opt-in health endpoint. Hand-rolled minimal HTTP over a
//! tokio listener — one endpoint does not justify a web framework. It
//! reports route names and loop states only: never payloads, never the
//! token. Route names are K8-restricted to `[A-Za-z0-9._-]`, so the
//! hand-built JSON cannot need escaping.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct HealthState {
    routes: Mutex<BTreeMap<String, &'static str>>,
}

impl HealthState {
    pub fn new(names: impl Iterator<Item = String>) -> Self {
        Self {
            routes: Mutex::new(names.map(|name| (name, "starting")).collect()),
        }
    }

    pub fn set(&self, route: &str, state: &'static str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            *slot = state;
        }
    }

    pub fn render(&self) -> String {
        let routes = self.routes.lock().expect("health lock");
        let items: Vec<String> = routes
            .iter()
            .map(|(name, state)| format!("{{\"name\":\"{name}\",\"state\":\"{state}\"}}"))
            .collect();
        format!("{{\"status\":\"ok\",\"routes\":[{}]}}", items.join(","))
    }
}

pub async fn serve(listener: tokio::net::TcpListener, state: Arc<HealthState>) {
    // Security F2: a liveness probe must not be the easiest way to
    // starve the process of fds — bound the concurrent connections,
    // time-limit each one, and never busy-spin on accept errors.
    let permits = Arc::new(tokio::sync::Semaphore::new(16));
    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(_) => {
                // EMFILE returns immediately; sleeping keeps fd
                // exhaustion from becoming a CPU spin as well.
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
            // Over the cap: drop the connection rather than queue it.
            continue;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let _permit = permit;
            let answer = async {
                // Drain whatever request line arrives; the answer is
                // the same for every path — a liveness probe, not an API.
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let body = state.render();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            };
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), answer).await;
        });
    }
}
