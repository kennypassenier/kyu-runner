//! The hub side of the pump: mailbox's three verbs over plain HTTP
//! (K2/K3 + W5 nack). Raw mode on purpose (AR4): the payload is the
//! body, byte for byte; metadata rides in `mailbox-*` headers.

use std::time::Duration;

use reqwest::{Client, StatusCode, header};
use thiserror::Error;

/// Settle calls must not dawdle: the lease budget reserves 5 s for
/// them (AR5).
const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);

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

    #[error("hub answered 200 without a {0} header — not a mailbox raw-mode response")]
    Protocol(&'static str),
}

/// One claimed message, exactly as the hub handed it over.
pub struct Claimed {
    pub id: String,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
    /// The `mailbox-*` metadata headers, passed through to HA (K2).
    pub meta: Vec<(String, String)>,
}

pub enum PollOutcome {
    Message(Box<Claimed>),
    Empty,
    /// 404 `UnknownTopic`: the topic has not been born yet — mailbox
    /// creates topics on first publish. A quiet wait state (AR6/AR16).
    TopicUnborn,
}

pub struct HubClient {
    poll: Client,
    settle: Client,
    base: String,
    token: Option<String>,
    wait_s: u64,
}

impl HubClient {
    pub fn new(base: &str, token: Option<String>, poll_wait: Duration) -> anyhow::Result<Self> {
        // The wire unit for `wait` is whole seconds (mailbox K2).
        let wait_s = poll_wait.as_secs().max(1);
        let poll = Client::builder()
            .timeout(Duration::from_secs(wait_s + 10))
            .build()?;
        let settle = Client::builder().timeout(SETTLE_TIMEOUT).build()?;
        Ok(Self {
            poll,
            settle,
            base: base.trim_end_matches('/').to_string(),
            token,
            wait_s,
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
                    .get("mailbox-id")
                    .and_then(|value| value.to_str().ok())
                {
                    Some(id) => id.to_string(),
                    None => return Err(HubError::Protocol("mailbox-id")),
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
                        name.as_str().starts_with("mailbox-") && name.as_str() != "mailbox-notice"
                    })
                    .filter_map(|(name, value)| {
                        value
                            .to_str()
                            .ok()
                            .map(|value| (name.as_str().to_string(), value.to_string()))
                    })
                    .collect();
                let body = response
                    .bytes()
                    .await
                    .map_err(|error| HubError::Unreachable(error.to_string()))?
                    .to_vec();
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
        Ok(text) if !text.is_empty() => text.chars().take(300).collect(),
        _ => "(no body)".into(),
    }
}
