//! Claiming the next message: the long-poll GET and the translation of
//! its raw-mode response (AR4) into a `PollOutcome`.

use reqwest::{Response, StatusCode, header};

use super::{Claimed, HubClient, HubError, PollOutcome, read_error_body};

impl HubClient {
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
            StatusCode::OK => self.read_claim(response).await,
            status => Err(HubError::Status {
                status,
                body: read_error_body(response).await,
            }),
        }
    }

    /// A 200: the id and metadata come from headers, the payload is the
    /// body byte for byte, read under the body cap.
    async fn read_claim(&self, mut response: Response) -> Result<PollOutcome, HubError> {
        let id = claim_id(&response)?;
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let meta = kyu_meta(&response);
        // Security F1: stream with a hard cap instead of buffering
        // whatever the hub decides to send.
        if let Some(length) = response.content_length()
            && length as usize > self.max_body_bytes
        {
            return Ok(PollOutcome::Oversize {
                id,
                at_least: length as usize,
                attempt: attempt_of(&meta),
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| HubError::Unreachable(error.to_string()))?
        {
            if body.len() + chunk.len() > self.max_body_bytes {
                return Ok(PollOutcome::Oversize {
                    id,
                    at_least: body.len() + chunk.len(),
                    attempt: attempt_of(&meta),
                });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(PollOutcome::Message(Box::new(Claimed {
            id,
            body,
            content_type,
            meta,
        })))
    }
}

/// Security F5: the id is interpolated into the settle URL path;
/// anything outside the plain charset is a protocol violation, not a
/// path.
fn claim_id(response: &Response) -> Result<String, HubError> {
    let id = response
        .headers()
        .get("kyu-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let plain = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if plain {
        Ok(id.to_string())
    } else {
        Err(HubError::Protocol("kyu-id"))
    }
}

/// The `kyu-*` metadata headers, passed through to HA (K2). `kyu-notice`
/// is the hub talking to the client, not message metadata.
fn kyu_meta(response: &Response) -> Vec<(String, String)> {
    response
        .headers()
        .iter()
        .filter(|(name, _)| name.as_str().starts_with("kyu-") && name.as_str() != "kyu-notice")
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn attempt_of(meta: &[(String, String)]) -> Option<String> {
    super::attempt_in(meta).map(str::to_owned)
}
