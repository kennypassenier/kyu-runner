//! The hub side of the pump: kyu's three verbs over plain HTTP
//! (K2/K3 + W5 nack). Raw mode on purpose (AR4): the payload is the
//! body, byte for byte; metadata rides in `kyu-*` headers.
//!
//! Layout: this file holds the types, the client and the settle and
//! policy calls; `claim` holds the long-poll that claims a message.

mod claim;

use std::time::Duration;

use reqwest::{Client, StatusCode, header, redirect};
use thiserror::Error;

/// Slack on top of the long-poll wait before the poll client gives up,
/// so a hub answering at the very end of its wait is not cut off.
const POLL_TIMEOUT_SLACK_S: u64 = 10;

/// Longest slice of a hub error body that reaches a log line.
const ERROR_BODY_LOG_CHARS: usize = 300;

#[derive(Debug, Error)]
pub enum HubError {
    #[error("hub unreachable: {0}")]
    Unreachable(String),

    #[error(
        "hub denied the request (401): the token is missing, wrong or revoked. Remedy: mint an \
         app token for \"kyu-runner\" on the hub's /apps page and set KYU_RUNNER_TOKEN in the \
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
    /// were discarded to keep the runner's memory bounded.
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
        // AR17 applies to this side too (security review, 2026-08-30).
        // reqwest follows redirects by default, so a compromised hub
        // could steer these calls at any LAN address. The token itself
        // is safe — reqwest strips `authorization` across hosts — but a
        // hub that answers 302 is malfunctioning either way, and saying
        // so out loud beats silently going somewhere else.
        let poll = Client::builder()
            .timeout(Duration::from_secs(wait_s + POLL_TIMEOUT_SLACK_S))
            .redirect(redirect::Policy::none())
            .build()?;
        let settle = Client::builder()
            .timeout(settle_timeout)
            .redirect(redirect::Policy::none())
            .build()?;
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
            // The answer only feeds the startup log; an unreadable one
            // says so there instead of passing for an empty policy.
            status if status.is_success() => Ok(response
                .text()
                .await
                .unwrap_or_else(|error| format!("(answer unreadable: {error})"))),
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
        Ok(text) if !text.is_empty() => printable(&text, ERROR_BODY_LOG_CHARS),
        _ => "(no body)".into(),
    }
}

/// Hub-controlled text, made safe for a log line: control characters
/// dropped, length bounded.
pub fn printable(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}
