//! Configuration: parse and validate, fail-closed (K8). Unknown keys,
//! duplicate routes and unusable values are startup errors, each with a
//! remedy in the message (standing rules 11/12). Pure — the only I/O is
//! the file read in `load`.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;

pub const DEFAULT_CONFIG_PATH: &str = "/etc/hub-bridge/config.toml";

/// The hub caps `wait` at 300 s (mailbox K2); staying under it keeps the
/// hub's answer authoritative instead of silently clamped.
const MAX_POLL_WAIT_MS: u64 = 300_000;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "cannot read config file {path}: {source}. Remedy: check that the file exists and is \
         readable; the default location is /etc/hub-bridge/config.toml and --config <path> \
         selects another."
    )]
    Unreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "config is not valid: {0}. Remedy: fix the TOML — unknown keys are refused on purpose, \
         so a typo fails loudly; compare with deploy/config.toml in the repo."
    )]
    Invalid(#[from] Box<toml::de::Error>),

    #[error(
        "no [[routes]] configured. Remedy: add at least one [[routes]] block with name, topic, \
         subscription and webhook_url — deploy/config.toml shows the shape."
    )]
    NoRoutes,

    #[error(
        "{field} is {url:?}, which does not start with http:// . Remedy: the bridge speaks plain \
         HTTP on the LAN only (AR11); use an http:// address such as http://127.0.0.1:8080."
    )]
    UrlScheme { field: String, url: String },

    #[error(
        "healthz_listen {value:?} is not a listen address. Remedy: use host:port, e.g. \
         \"0.0.0.0:8081\" — or remove the key to keep the health endpoint off."
    )]
    HealthzListen { value: String },

    #[error(
        "[defaults] poll_wait_ms is {value} ms, outside 1000-300000. Remedy: pick a value \
         between 1 s and the hub's 300 s long-poll cap; the default of 25000 is fine for \
         almost everything."
    )]
    PollWait { value: u64 },

    #[error(
        "{scope} webhook_timeout_ms is {value} ms, below 100. Remedy: give HA at least 100 ms; \
         the default of 10000 is fine for almost everything."
    )]
    WebhookTimeout { scope: String, value: u64 },

    #[error("route {name:?}: {problem} Remedy: {remedy}")]
    Route {
        name: String,
        problem: String,
        remedy: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub hub_url: String,
    /// W4: absent means no health socket is ever opened (fail-closed).
    #[serde(default)]
    pub healthz_listen: Option<String>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub routes: Vec<Route>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    pub poll_wait_ms: u64,
    pub webhook_timeout_ms: u64,
    /// AR9: how long an in-flight delivery may finish after SIGTERM.
    pub shutdown_grace_ms: u64,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            poll_wait_ms: 25_000,
            webhook_timeout_ms: 10_000,
            shutdown_grace_ms: 10_000,
        }
    }
}

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
    /// The bridge does not interpret it — the hub validates policies and
    /// refuses unusable ones with a remedy (mailbox K7), so hardcoding
    /// the hub's field names here would only add a second, staler copy.
    #[serde(default)]
    pub policy: Option<toml::Table>,
}

impl Config {
    pub fn webhook_timeout(&self, route: &Route) -> Duration {
        Duration::from_millis(
            route
                .webhook_timeout_ms
                .unwrap_or(self.defaults.webhook_timeout_ms),
        )
    }

    pub fn poll_wait(&self) -> Duration {
        Duration::from_millis(self.defaults.poll_wait_ms)
    }

    pub fn shutdown_grace(&self) -> Duration {
        Duration::from_millis(self.defaults.shutdown_grace_ms)
    }

    fn validate(&mut self) -> Result<(), ConfigError> {
        while self.hub_url.ends_with('/') {
            self.hub_url.pop();
        }
        if !self.hub_url.starts_with("http://") {
            return Err(ConfigError::UrlScheme {
                field: "hub_url".into(),
                url: self.hub_url.clone(),
            });
        }
        if let Some(listen) = &self.healthz_listen
            && listen.parse::<SocketAddr>().is_err()
        {
            return Err(ConfigError::HealthzListen {
                value: listen.clone(),
            });
        }
        if !(1_000..=MAX_POLL_WAIT_MS).contains(&self.defaults.poll_wait_ms) {
            return Err(ConfigError::PollWait {
                value: self.defaults.poll_wait_ms,
            });
        }
        if self.defaults.webhook_timeout_ms < 100 {
            return Err(ConfigError::WebhookTimeout {
                scope: "[defaults]".into(),
                value: self.defaults.webhook_timeout_ms,
            });
        }
        if self.routes.is_empty() {
            return Err(ConfigError::NoRoutes);
        }

        let mut names = HashSet::new();
        let mut pairs = HashSet::new();
        for route in &self.routes {
            route.validate()?;
            if let Some(value) = route.webhook_timeout_ms
                && value < 100
            {
                return Err(ConfigError::WebhookTimeout {
                    scope: format!("route {:?}", route.name),
                    value,
                });
            }
            if !names.insert(route.name.clone()) {
                return Err(ConfigError::Route {
                    name: route.name.clone(),
                    problem: "its name is used by another route.".into(),
                    remedy: "route names are the log and health handles, so give every \
                             [[routes]] block its own."
                        .into(),
                });
            }
            if !pairs.insert((route.topic.clone(), route.subscription.clone())) {
                return Err(ConfigError::Route {
                    name: route.name.clone(),
                    problem: format!(
                        "another route already consumes topic {:?} as subscription {:?} — the \
                         two would compete for the same messages and split them between \
                         webhooks.",
                        route.topic, route.subscription
                    ),
                    remedy: "give this route its own subscription name (fan-out: each name \
                             receives every message) or remove the duplicate."
                        .into(),
                });
            }
        }
        Ok(())
    }
}

impl Route {
    fn validate(&self) -> Result<(), ConfigError> {
        let ident = |value: &str| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        };
        let err = |problem: &str, remedy: &str| {
            Err(ConfigError::Route {
                name: self.name.clone(),
                problem: problem.into(),
                remedy: remedy.into(),
            })
        };
        if !ident(&self.name) {
            return err(
                "the name is empty or contains characters outside A-Za-z0-9._- .",
                "route names end up in logs and the health endpoint; keep them plain, \
                 e.g. \"mailbox-events\".",
            );
        }
        if !ident(&self.topic) {
            return err(
                "the topic is empty or contains characters outside A-Za-z0-9._- .",
                "house topics are flat <domain>.<qualifier> names (study §7), \
                 e.g. \"homelab.ops\".",
            );
        }
        if !ident(&self.subscription) {
            return err(
                "the subscription is empty or contains characters outside A-Za-z0-9._- .",
                "subscription names are mailbox's one concept; keep them plain, \
                 e.g. \"ha-bridge\".",
            );
        }
        if !self.webhook_url.starts_with("http://") {
            return Err(ConfigError::UrlScheme {
                field: format!("route {:?} webhook_url", self.name),
                url: self.webhook_url.clone(),
            });
        }
        Ok(())
    }
}

pub fn load(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Unreadable {
        path: path.display().to_string(),
        source,
    })?;
    parse(&text)
}

pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let mut config: Config = toml::from_str(text).map_err(Box::new)?;
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        hub_url = "http://127.0.0.1:8080/"

        [[routes]]
        name = "mailbox-events"
        topic = "mailbox.events"
        subscription = "ha-bridge"
        webhook_url = "http://ha.lan:8123/api/webhook/hub_mailbox_events"
    "#;

    fn remedy_of(error: ConfigError) -> String {
        let text = error.to_string();
        assert!(
            text.contains("Remedy:"),
            "K8/rule 11: every config error carries a remedy — got: {text}"
        );
        text
    }

    #[test]
    fn l1_k8_a_valid_config_parses_and_normalizes() {
        let config = parse(VALID).expect("valid config");
        assert_eq!(config.hub_url, "http://127.0.0.1:8080");
        assert_eq!(config.defaults.poll_wait_ms, 25_000);
        assert_eq!(
            config.webhook_timeout(&config.routes[0]),
            Duration::from_millis(10_000)
        );
        assert!(config.healthz_listen.is_none());
    }

    #[test]
    fn l1_k8_a_per_route_timeout_overrides_the_default() {
        let text = VALID.replace(
            "webhook_url =",
            "webhook_timeout_ms = 5000\n        webhook_url =",
        );
        let config = parse(&text).expect("valid config");
        assert_eq!(
            config.webhook_timeout(&config.routes[0]),
            Duration::from_millis(5_000)
        );
    }

    #[test]
    fn l1_k8_an_unknown_key_is_refused_with_a_remedy() {
        let text = VALID.replace("hub_url", "hub_uri");
        let message = remedy_of(parse(&text).expect_err("typo must fail loudly"));
        assert!(message.contains("unknown keys are refused"), "{message}");
    }

    #[test]
    fn l1_k8_no_routes_is_refused_with_a_remedy() {
        let message = remedy_of(parse("hub_url = \"http://h:1\"").expect_err("no routes"));
        assert!(message.contains("[[routes]]"), "{message}");
    }

    #[test]
    fn l1_k8_a_non_http_hub_url_is_refused() {
        let text = VALID.replace("http://127.0.0.1:8080/", "https://127.0.0.1:8080");
        let message = remedy_of(parse(&text).expect_err("https must be refused"));
        assert!(message.contains("plain HTTP"), "{message}");
    }

    #[test]
    fn l1_k8_a_non_http_webhook_url_is_refused() {
        let text = VALID.replace("http://ha.lan", "ftp://ha.lan");
        remedy_of(parse(&text).expect_err("ftp webhook must be refused"));
    }

    #[test]
    fn l1_k8_duplicate_route_names_are_refused() {
        let text = format!(
            "{VALID}\n[[routes]]\nname = \"mailbox-events\"\ntopic = \"t2\"\n\
             subscription = \"s2\"\nwebhook_url = \"http://ha.lan:8123/api/webhook/x\"\n"
        );
        let message = remedy_of(parse(&text).expect_err("dup name"));
        assert!(message.contains("used by another route"), "{message}");
    }

    #[test]
    fn l1_k8_a_duplicate_topic_subscription_pair_is_refused() {
        let text = format!(
            "{VALID}\n[[routes]]\nname = \"second\"\ntopic = \"mailbox.events\"\n\
             subscription = \"ha-bridge\"\nwebhook_url = \"http://ha.lan:8123/api/webhook/x\"\n"
        );
        let message = remedy_of(parse(&text).expect_err("dup pair"));
        assert!(message.contains("compete"), "{message}");
    }

    #[test]
    fn l1_k8_bad_identifier_characters_are_refused() {
        for (field, bad) in [
            ("name = \"mailbox-events\"", "name = \"mail box\""),
            ("topic = \"mailbox.events\"", "topic = \"mailbox/events\""),
            ("subscription = \"ha-bridge\"", "subscription = \"\""),
        ] {
            let text = VALID.replace(field, bad);
            remedy_of(parse(&text).expect_err(bad));
        }
    }

    #[test]
    fn l1_k8_poll_wait_outside_the_hub_cap_is_refused() {
        for value in ["500", "300001"] {
            let text = format!("{VALID}\n[defaults]\npoll_wait_ms = {value}\n");
            let message = remedy_of(parse(&text).expect_err("bad poll wait"));
            assert!(message.contains("300 s"), "{message}");
        }
    }

    #[test]
    fn l1_k8_a_tiny_webhook_timeout_is_refused() {
        let text = format!("{VALID}\n[defaults]\nwebhook_timeout_ms = 50\n");
        remedy_of(parse(&text).expect_err("tiny timeout"));
    }

    #[test]
    fn l1_w4_healthz_listen_must_be_a_socket_address() {
        let text = VALID.replace(
            "hub_url =",
            "healthz_listen = \"not-an-address\"\nhub_url =",
        );
        let message = remedy_of(parse(&text).expect_err("bad listen addr"));
        assert!(message.contains("host:port"), "{message}");
    }

    #[test]
    fn l1_w3_a_policy_block_is_carried_verbatim() {
        let text = VALID.replace(
            "webhook_url =",
            "policy = { max_attempts = 10 }\n        webhook_url =",
        );
        let config = parse(&text).expect("valid config");
        let policy = config.routes[0].policy.as_ref().expect("policy present");
        assert_eq!(
            policy.get("max_attempts").and_then(|v| v.as_integer()),
            Some(10)
        );
    }
}
