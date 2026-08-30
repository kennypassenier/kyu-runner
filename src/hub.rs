//! The hub side of the pump: kyu's three verbs over plain HTTP
//! (K2/K3 + W5 nack). Raw mode on purpose (AR4): the payload is the
//! body, byte for byte; metadata rides in `kyu-*` headers.

use std::time::Duration;

use reqwest::{Client, StatusCode, header};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HubError {
    #[error("hub unreachable: {0}")]
    Unreachable(String),

    #[error(
        "hub denied the request (401): the token is missing, wrong or revoked. Remedy: mint an \
         app token for \"hub-bridge\" on the hub's /apps page and set HUB_BRIDGE_TOKEN in the \
         unit's EnvironmentFile."
    )]
    Auth,

    #[error("hub answered {status}: {body}")]
    Status { status: StatusCode, body: String },

    #[error("hub answered 200 without a {0} header — not a kyu raw-mode response")]
    Protocol(&'static str),
}

/// One claimed message, exactly as the hub handed it over.
pub struct Claimed {
    pub id: String,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    /// The `kyu-*` metadata headers, passed through to HA (K2).
    pub meta: Vec<(String, String)>,
}

pub enum PollOutcome {
    Message(Box<Claimed>),
    Empty,
    /// 404 `UnknownTopic`: the topic has not been born yet — kyu
    /// creates topics on first publish. A quiet wait state (AR6/AR16).
    TopicUnborn,
    /// Security F1: the body outgrew the configured cap while being
    /// read. The claim is real (the id is here to nack it); the bytes
    /// were discarded to keep the bridge's memory bounded.
    Oversize {
        id: String,
        at_least: usize,
    },
}

pub struct HubClient {
    poll: Client,
    settle: Client,
    base: String,
    token: Option<String>,
    wait_s: u64,
    max_body_bytes: usize,
}

impl HubClient {
    pub fn new(
        base: &str,
        token: Option<String>,
        poll_wait: Duration,
        max_body_bytes: u64,
        settle_timeout: Duration,
    ) -> anyhow::Result<Self> {
        // The wire unit for `wait` is whole seconds (kyu K2).
        let wait_s = poll_wait.as_secs().max(1);
        let poll = Client::builder()
            .timeout(Duration::from_secs(wait_s + 10))
            .build()?;
        let settle = Client::builder().timeout(settle_timeout).build()?;
        Ok(Self {
            poll,
            settle,
            base: base.trim_end_matches('/').to_string(),
            token,
            wait_s,
            max_body_bytes: max_body_bytes as usize,
        })
    }

    fn authed(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    pub async fn next(
        &self,
        topic: &str,
        subscription: &str,
        from_beginning: bool,
    ) -> Result<PollOutcome, HubError> {
        let mut url = format!(
            "{}/t/{topic}/next?as={subscription}&wait={}",
            self.base, self.wait_s
        );
        if from_beginning {
            url.push_str("&from=beginning");
        }
        let response = self
            .authed(self.poll.get(url))
            .send()
            .await
            .map_err(|error| HubError::Unreachable(error.to_string()))?;

        match response.status() {
            StatusCode::NO_CONTENT => Ok(PollOutcome::Empty),
            StatusCode::NOT_FOUND => Ok(PollOutcome::TopicUnborn),
            StatusCode::UNAUTHORIZED => Err(HubError::Auth),
            StatusCode::OK => {
                let id = match response
                    .headers()
                    .get("kyu-id")
                    .and_then(|value| value.to_str().ok())
                {
                    // Security F5: the id is interpolated into the
                    // settle URL path; anything outside the plain
                    // charset is a protocol violation, not a path.
                    Some(id)
                        if !id.is_empty()
                            && id
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) =>
                    {
                        id.to_string()
                    }
                    _ => return Err(HubError::Protocol("kyu-id")),
                };
                let content_type = response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                let meta = response
                    .headers()
                    .iter()
                    .filter(|(name, _)| {
                        name.as_str().starts_with("kyu-") && name.as_str() != "kyu-notice"
                    })
                    .filter_map(|(name, value)| {
                        value
                            .to_str()
                            .ok()
                            .map(|value| (name.as_str().to_string(), value.to_string()))
                    })
                    .collect();
                // Security F1: stream with a hard cap instead of
                // buffering whatever the hub decides to send.
                if let Some(length) = response.content_length()
                    && length as usize > self.max_body_bytes
                {
                    return Ok(PollOutcome::Oversize {
                        id,
                        at_least: length as usize,
                    });
                }
                let mut body = Vec::new();
                let mut response = response;
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            if body.len() + chunk.len() > self.max_body_bytes {
                                return Ok(PollOutcome::Oversize {
                                    id,
                                    at_least: body.len() + chunk.len(),
                                });
                            }
                            body.extend_from_slice(&chunk);
                        }
                        Ok(None) => break,
                        Err(error) => return Err(HubError::Unreachable(error.to_string())),
                    }
                }
                Ok(PollOutcome::Message(Box::new(Claimed {
                    id,
                    body,
                    content_type,
                    meta,
                })))
            }
            status => Err(HubError::Status {
                status,
                body: read_error_body(response).await,
            }),
        }
    }

    pub async fn ack(&self, topic: &str, subscription: &str, id: &str) -> Result<(), HubError> {
        self.settle_call("ack", topic, subscription, id, "").await
    }

    pub async fn nack(&self, topic: &str, subscription: &str, id: &str) -> Result<(), HubError> {
        self.settle_call("nack", topic, subscription, id, "").await
    }

    async fn settle_call(
        &self,
        verb: &str,
        topic: &str,
        subscription: &str,
        id: &str,
        query_extra: &str,
    ) -> Result<(), HubError> {
        let url = format!(
            "{}/t/{topic}/{verb}/{id}?as={subscription}{query_extra}",
            self.base
        );
        let response = self
            .authed(self.settle.post(url))
            .send()
            .await
            .map_err(|error| HubError::Unreachable(error.to_string()))?;
        match response.status() {
            status if status.is_success() => Ok(()),
            StatusCode::UNAUTHORIZED => Err(HubError::Auth),
            status => Err(HubError::Status {
                status,
                body: read_error_body(response).await,
            }),
        }
    }

    /// W3: forward a route's policy block to the hub. Returns the hub's
    /// answer (the values in force) for the startup log.
    pub async fn put_policy(
        &self,
        topic: &str,
        subscription: &str,
        policy_json: &str,
    ) -> Result<String, HubError> {
        let url = format!("{}/api/t/{topic}/subs/{subscription}/policy", self.base);
        let response = self
            .authed(self.settle.put(url))
            .header(header::CONTENT_TYPE, "application/json")
            .body(policy_json.to_string())
            .send()
            .await
            .map_err(|error| HubError::Unreachable(error.to_string()))?;
        match response.status() {
            status if status.is_success() => {
                Ok(response.text().await.unwrap_or_else(|_| "{}".into()))
            }
            StatusCode::UNAUTHORIZED => Err(HubError::Auth),
            status => Err(HubError::Status {
                status,
                body: read_error_body(response).await,
            }),
        }
    }
}

async fn read_error_body(response: reqwest::Response) -> String {
    match response.text().await {
        // Security F4: this string ends up in log lines — strip control
        // characters so a hostile hub cannot forge log entries or
        // inject terminal escapes via its error bodies.
        Ok(text) if !text.is_empty() => printable(&text, 300),
        _ => "(no body)".into(),
    }
}

/// Hub-controlled text, made safe for a log line: control characters
/// dropped, length bounded.
pub fn printable(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}
