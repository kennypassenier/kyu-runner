//! The HA side of the pump: one POST per message, byte for byte (K2),
//! no redirects (AR17 — a followed 302 becomes a body-less GET that HA
//! may answer 200, acking a payload that never arrived).

use std::time::Duration;

use reqwest::{Client, StatusCode, header, redirect};

use crate::hub::Claimed;

#[derive(Debug)]
pub enum DeliveryError {
    /// The target machine is down: connection refused/timed out. Opens
    /// the route's circuit breaker (AR15) — nothing payload-specific.
    ConnectClass(String),
    /// The target's HTTP stack answered, but not 2xx (3xx included).
    Status(StatusCode),
}

impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConnectClass(reason) => write!(f, "target unreachable: {reason}"),
            Self::Status(status) => write!(f, "target answered {status}"),
        }
    }
}

pub struct WebhookClient {
    http: Client,
}

impl WebhookClient {
    pub fn new() -> anyhow::Result<Self> {
        let http = Client::builder()
            .redirect(redirect::Policy::none())
            .build()?;
        Ok(Self { http })
    }

    pub async fn deliver(
        &self,
        url: &str,
        timeout: Duration,
        message: &Claimed,
    ) -> Result<(), DeliveryError> {
        let mut request = self
            .http
            .post(url)
            .timeout(timeout)
            .body(message.body.clone());
        if let Some(content_type) = &message.content_type {
            request = request.header(header::CONTENT_TYPE, content_type);
        }
        for (name, value) in &message.meta {
            request = request.header(name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|error| DeliveryError::ConnectClass(error.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(DeliveryError::Status(response.status()))
        }
    }
}

/// AR15's probe: a bare TCP connect to the webhook's host:port — no
/// HTTP request is ever sent, so no webhook can be triggered by
/// probing.
pub async fn probe_origin(url: &str, timeout: Duration) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let port = parsed.port_or_known_default().unwrap_or(80);
    matches!(
        tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}
