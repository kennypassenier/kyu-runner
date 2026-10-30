//! One `[[routes]]` block: which topic and subscription to consume and
//! which webhook receives the messages. The checks here concern a single
//! route on its own; checks that compare routes or need the defaults
//! live in `validate`.

use serde::Deserialize;

use super::ConfigError;
use super::validate::validate_url;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Route {
    /// The log/health handle for this route; unique, `[A-Za-z0-9._-]`.
    pub name: String,
    pub topic: String,
    pub subscription: String,
    pub webhook_url: String,
    #[serde(default)]
    pub webhook_timeout_ms: Option<u64>,
    /// W3: forwarded verbatim to the hub's policy endpoint at startup.
    /// The runner does not interpret it — the hub validates policies and
    /// refuses unusable ones with a remedy (kyu K7), so hardcoding
    /// the hub's field names here would only add a second, staler copy.
    #[serde(default)]
    pub policy: Option<toml::Table>,
}

/// A plain identifier: non-empty and only `A-Za-z0-9._-`, so it is safe
/// in log lines, health keys and hub URL paths alike.
fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}

impl Route {
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        let checks = [
            (
                &self.name,
                "the name is empty or contains characters outside A-Za-z0-9._- .",
                "route names end up in logs and the health endpoint; keep them plain, \
                 e.g. \"kyu-events\".",
            ),
            (
                &self.topic,
                "the topic is empty or contains characters outside A-Za-z0-9._- .",
                "house topics are flat <domain>.<qualifier> names (study §7), \
                 e.g. \"homelab.ops\".",
            ),
            (
                &self.subscription,
                "the subscription is empty or contains characters outside A-Za-z0-9._- .",
                "subscription names are kyu's one concept; keep them plain, \
                 e.g. \"ha-runner\".",
            ),
        ];
        for (value, problem, remedy) in checks {
            if !is_identifier(value) {
                return Err(self.error(problem, remedy));
            }
        }
        validate_url(
            &format!("route {:?} webhook_url", self.name),
            &self.webhook_url,
        )
    }

    /// A route-scoped error carrying this route's name.
    pub(super) fn error(&self, problem: impl Into<String>, remedy: &str) -> ConfigError {
        ConfigError::Route {
            name: self.name.clone(),
            problem: problem.into(),
            remedy: remedy.into(),
        }
    }
}
