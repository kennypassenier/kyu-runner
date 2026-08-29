//! W4/AR10 + W6: the opt-in observation socket. Hand-rolled minimal
//! HTTP over a tokio listener — two endpoints do not justify a web
//! framework. `/healthz` reports route names and loop states,
//! `/metrics` the W6 Prometheus counters; never payloads, never the
//! token. Route names are K8-restricted to `[A-Za-z0-9._-]`, so the
//! hand-built JSON and metric labels cannot need escaping.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct RouteSlot {
    state: &'static str,
    delivered: u64,
    nacked: u64,
}

pub struct HealthState {
    routes: Mutex<BTreeMap<String, RouteSlot>>,
}

impl HealthState {
    pub fn new(names: impl Iterator<Item = String>) -> Self {
        Self {
            routes: Mutex::new(
                names
                    .map(|name| {
                        (
                            name,
                            RouteSlot {
                                state: "starting",
                                delivered: 0,
                                nacked: 0,
                            },
                        )
                    })
                    .collect(),
            ),
        }
    }

    pub fn set(&self, route: &str, state: &'static str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.state = state;
        }
    }

    /// W6: a message reached the webhook (2xx) and was settled.
    pub fn count_delivered(&self, route: &str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.delivered += 1;
        }
    }

    /// W6: a delivery was handed back to the hub (any nack).
    pub fn count_nacked(&self, route: &str) {
        if let Some(slot) = self.routes.lock().expect("health lock").get_mut(route) {
            slot.nacked += 1;
        }
    }

    pub fn render_health(&self) -> String {
        let routes = self.routes.lock().expect("health lock");
        let items: Vec<String> = routes
            .iter()
            .map(|(name, slot)| format!("{{\"name\":\"{name}\",\"state\":\"{}\"}}", slot.state))
            .collect();
        format!("{{\"status\":\"ok\",\"routes\":[{}]}}", items.join(","))
    }

    /// W6: Prometheus text format, counters only.
    pub fn render_metrics(&self) -> String {
        let routes = self.routes.lock().expect("health lock");
        let mut out = String::from(
            "# HELP hub_bridge_delivered_total Messages delivered to the webhook and settled.\n\
             # TYPE hub_bridge_delivered_total counter\n",
        );
        for (name, slot) in routes.iter() {
            out.push_str(&format!(
                "hub_bridge_delivered_total{{route=\"{name}\"}} {}\n",
                slot.delivered
            ));
        }
        out.push_str(
            "# HELP hub_bridge_nacked_total Deliveries handed back to the hub (nacked).\n\
             # TYPE hub_bridge_nacked_total counter\n",
        );
        for (name, slot) in routes.iter() {
            out.push_str(&format!(
                "hub_bridge_nacked_total{{route=\"{name}\"}} {}\n",
                slot.nacked
            ));
        }
        out
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
                // One read is enough for a GET's request line; the
                // path picks the endpoint, everything else is ignored.
                let mut buffer = [0u8; 1024];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]);
                let path = head
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/healthz")
                    .to_string();
                let (body, content_type) = if path.starts_with("/metrics") {
                    (
                        state.render_metrics(),
                        "text/plain; version=0.0.4; charset=utf-8",
                    )
                } else {
                    (state.render_health(), "application/json")
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            };
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), answer).await;
        });
    }
}
