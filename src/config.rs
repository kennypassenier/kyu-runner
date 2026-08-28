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

/// The hub's default lease when a route sets no policy of its own
/// (mailbox `DEFAULT_LEASE_MS`). If mailbox ever changes this default,
/// routes without an explicit `policy.lease_ms` get a wrong budget —
/// which is why the K8 remedy pushes toward setting one.
const HUB_DEFAULT_LEASE_MS: u64 = 30_000;

/// Time reserved inside the lease for settling (ack/nack) after the
/// last delivery attempt (AR5).
const SETTLE_MARGIN_MS: u64 = 5_000;

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

    #[error(
        "route {name:?}: webhook_timeout_ms {timeout} + {margin} ms settle margin does not fit \
         the lease budget of {budget} ms (70% of the {lease} ms lease). Remedy: lower the \
         timeout, or raise the lease by setting policy.lease_ms on this route (W3) — the \
         delivery must always be settled by its owner, never by lease expiry (AR5)."
    )]
    LeaseBudget {
        name: String,
        timeout: u64,
        margin: u64,
        budget: u64,
        lease: u64,
    },

    #[error(
        "route {name:?}: policy.lease_ms is {value}, not an integer between 1000 and 86400000 \
         (1 s - 24 h). Remedy: give the lease in milliseconds, e.g. lease_ms = 300000 for five \
         minutes."
    )]
    PolicyLease { name: String, value: String },

    #[error("route {name:?}: {problem} Remedy: {remedy}")]
    Route {
        name: String,
        problem: String,
        remedy: String,
    },

    #[error(
        "{field} is {url:?}, which is not a usable http URL ({reason}). Remedy: use a full \
         http://host[:port]/path address without credentials — the bridge sends its token in a \
         header, never in a URL."
    )]
    UrlInvalid {
        field: String,
        url: String,
        reason: String,
    },

    #[error(
        "[defaults] max_body_bytes is {value}, below 1024. Remedy: the cap protects the bridge \
         from buffering runaway messages; anything from 1024 up is accepted, the default is \
         16 MiB."
    )]
    BodyCap { value: u64 },
}

/// Security F6: a prefix check alone let `http://` through, which then
/// wedged a route in circuit-open forever at runtime. Parse for real.
fn validate_url(field: &str, url: &str) -> Result<(), ConfigError> {
    if !url.starts_with("http://") {
        return Err(ConfigError::UrlScheme {
            field: field.into(),
            url: url.into(),
        });
    }
    let invalid = |reason: &str| ConfigError::UrlInvalid {
        field: field.into(),
        url: url.into(),
        reason: reason.into(),
    };
    let parsed = reqwest::Url::parse(url).map_err(|error| invalid(&error.to_string()))?;
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(invalid("it has no host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("it carries credentials (userinfo)"));
    }
    Ok(())
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
    /// Security F1: a claimed message is buffered in memory before the
    /// webhook POST; this caps it so a misbehaving hub cannot OOM the
    /// bridge. An oversize message is nacked and dead-letters visibly.
    pub max_body_bytes: u64,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            poll_wait_ms: 25_000,
            webhook_timeout_ms: 10_000,
            max_body_bytes: 16 * 1024 * 1024,
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

    /// AR9: derived, not configured — the longest webhook timeout plus
    /// the settle margin, so an in-flight delivery always fits.
    pub fn shutdown_grace(&self) -> Duration {
        let max_timeout = self
            .routes
            .iter()
            .map(|route| self.webhook_timeout(route).as_millis() as u64)
            .max()
            .unwrap_or(self.defaults.webhook_timeout_ms);
        Duration::from_millis(max_timeout + SETTLE_MARGIN_MS)
    }

    /// The lease this route effectively runs under: its own
    /// `policy.lease_ms` if set, else the hub default.
    pub fn effective_lease_ms(route: &Route) -> u64 {
        route
            .policy
            .as_ref()
            .and_then(|policy| policy.get("lease_ms"))
            .and_then(|value| value.as_integer())
            .filter(|value| *value > 0)
            .map(|value| value as u64)
            .unwrap_or(HUB_DEFAULT_LEASE_MS)
    }

    /// AR5: in-process retries stop at 70% of the effective lease, so
    /// the claim is always settled by its owner, never by expiry.
    pub fn lease_budget(&self, route: &Route) -> Duration {
        Duration::from_millis(Self::effective_lease_ms(route) * 7 / 10)
    }

    /// W3: the policy block as the JSON document the hub expects. The
    /// bridge does not know the hub's field names on purpose (the hub
    /// validates and refuses with a remedy); it only guarantees the
    /// value types survive the TOML→JSON trip, which K8 validation
    /// restricts to integers, strings and booleans.
    pub fn policy_json(route: &Route) -> Option<String> {
        let table = route.policy.as_ref()?;
        let fields: Vec<String> = table
            .iter()
            .map(|(key, value)| {
                let rendered = match value {
                    toml::Value::Integer(number) => number.to_string(),
                    toml::Value::Boolean(flag) => flag.to_string(),
                    toml::Value::String(text) => format!("{text:?}"),
                    other => other.to_string(),
                };
                format!("{key:?}:{rendered}")
            })
            .collect();
        Some(format!("{{{}}}", fields.join(",")))
    }

    fn validate(&mut self) -> Result<(), ConfigError> {
        while self.hub_url.ends_with('/') {
            self.hub_url.pop();
        }
        validate_url("hub_url", &self.hub_url)?;
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
        if self.defaults.max_body_bytes < 1024 {
            return Err(ConfigError::BodyCap {
                value: self.defaults.max_body_bytes,
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
            if let Some(policy) = &route.policy {
                for (key, value) in policy {
                    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        return Err(ConfigError::Route {
                            name: route.name.clone(),
                            problem: format!("policy key {key:?} is not a plain field name."),
                            remedy: "policy keys are the hub's snake_case fields, e.g. lease_ms, \
                                     max_attempts."
                                .into(),
                        });
                    }
                    // Security F7: the policy JSON is hand-built, so
                    // values that would need JSON escaping are refused
                    // rather than mis-encoded (which would make the PUT
                    // fail forever at runtime).
                    let clean = match value {
                        toml::Value::Integer(_) | toml::Value::Boolean(_) => true,
                        toml::Value::String(text) => text
                            .bytes()
                            .all(|b| (0x20..=0x7e).contains(&b) && b != b'"' && b != b'\\'),
                        _ => false,
                    };
                    if !clean {
                        return Err(ConfigError::Route {
                            name: route.name.clone(),
                            problem: format!(
                                "policy.{key} is {value}, which is not an integer, boolean or \
                                 plain ASCII string."
                            ),
                            remedy: "policy blocks are flat hub fields, e.g. lease_ms = 300000, \
                                     max_attempts = 25, ttl_ms = \"never\" — see the hub's \
                                     policy endpoint for what it accepts."
                                .into(),
                        });
                    }
                }
            }
            if let Some(lease) = route.policy.as_ref().and_then(|p| p.get("lease_ms"))
                && !lease
                    .as_integer()
                    .is_some_and(|value| (1_000..=86_400_000).contains(&value))
            {
                return Err(ConfigError::PolicyLease {
                    name: route.name.clone(),
                    value: lease.to_string(),
                });
            }
            let timeout = route
                .webhook_timeout_ms
                .unwrap_or(self.defaults.webhook_timeout_ms);
            let lease = Self::effective_lease_ms(route);
            let budget = lease * 7 / 10;
            if timeout + SETTLE_MARGIN_MS > budget {
                return Err(ConfigError::LeaseBudget {
                    name: route.name.clone(),
                    timeout,
                    margin: SETTLE_MARGIN_MS,
                    budget,
                    lease,
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
        validate_url(
            &format!("route {:?} webhook_url", self.name),
            &self.webhook_url,
        )?;
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
    fn l1_ar5_a_webhook_timeout_that_outgrows_the_lease_budget_is_refused() {
        let text = VALID.replace(
            "webhook_url =",
            "webhook_timeout_ms = 17000\n        webhook_url =",
        );
        let message = remedy_of(parse(&text).expect_err("17s + 5s > 21s budget"));
        assert!(message.contains("policy.lease_ms"), "{message}");
    }

    #[test]
    fn l1_ar5_a_raised_policy_lease_widens_the_budget() {
        let text = VALID.replace(
            "webhook_url =",
            "webhook_timeout_ms = 17000\n        policy = { lease_ms = 60000 }\n        webhook_url =",
        );
        let config = parse(&text).expect("60s lease gives a 42s budget");
        assert_eq!(
            config.lease_budget(&config.routes[0]),
            Duration::from_millis(42_000)
        );
    }

    #[test]
    fn l1_ar5_a_non_integer_policy_lease_is_refused() {
        let text = VALID.replace(
            "webhook_url =",
            "policy = { lease_ms = \"long\" }\n        webhook_url =",
        );
        remedy_of(parse(&text).expect_err("string lease must be refused"));
    }

    #[test]
    fn l1_ar9_the_shutdown_grace_is_derived_from_the_largest_timeout() {
        let text = format!(
            "{VALID}\n[[routes]]\nname = \"slow\"\ntopic = \"t2\"\nsubscription = \"s2\"\n\
             webhook_timeout_ms = 12000\npolicy = {{ lease_ms = 30000 }}\n\
             webhook_url = \"http://ha.lan:8123/api/webhook/slow\"\n"
        );
        let config = parse(&text).expect("valid config");
        assert_eq!(config.shutdown_grace(), Duration::from_millis(17_000));
    }

    #[test]
    fn l1_ar5_an_absurd_policy_lease_is_refused_not_wrapped() {
        // Security review F7: lease_ms * 7 must never overflow into a
        // "valid" budget.
        let text = VALID.replace(
            "webhook_url =",
            "policy = { lease_ms = 9223372036854775 }\n        webhook_url =",
        );
        remedy_of(parse(&text).expect_err("absurd lease must be refused"));
    }

    #[test]
    fn l1_k8_a_policy_string_needing_json_escapes_is_refused() {
        // Security review F7: policy JSON is hand-built; values that
        // would need escaping are refused instead of mis-encoded.
        let text = VALID.replace(
            "webhook_url =",
            "policy = { ttl_ms = \"nev\\\"er\" }\n        webhook_url =",
        );
        remedy_of(parse(&text).expect_err("quote in policy string must be refused"));
    }

    #[test]
    fn l1_k8_an_unparsable_webhook_url_is_refused_with_a_remedy() {
        // Security review F6: "http://" passes a prefix check, then
        // wedges the route in a permanent circuit-open state at runtime.
        let text = VALID.replace(
            "webhook_url = \"http://ha.lan:8123/api/webhook/hub_mailbox_events\"",
            "webhook_url = \"http://\"",
        );
        remedy_of(parse(&text).expect_err("hostless url must be refused"));
    }

    #[test]
    fn l1_k8_a_userinfo_url_is_refused() {
        let text = VALID.replace("http://ha.lan:8123", "http://user:pw@ha.lan:8123");
        remedy_of(parse(&text).expect_err("userinfo must be refused"));
    }

    #[test]
    fn l1_k8_a_tiny_body_cap_is_refused() {
        let text = format!("{VALID}\n[defaults]\nmax_body_bytes = 100\n");
        remedy_of(parse(&text).expect_err("cap below 1024 must be refused"));
    }

    #[test]
    fn l1_k8_a_per_route_tiny_webhook_timeout_is_refused() {
        let text = VALID.replace(
            "webhook_url =",
            "webhook_timeout_ms = 50\n        webhook_url =",
        );
        let message = remedy_of(parse(&text).expect_err("per-route tiny timeout"));
        assert!(message.contains("mailbox-events"), "{message}");
    }

    #[test]
    fn l1_k8_a_policy_value_of_a_refused_type_is_refused() {
        for bad in [
            "policy = { lease_ms = 1.5 }",
            "policy = { steps = [1, 2] }",
            "policy = { nested = { a = 1 } }",
        ] {
            let text = VALID.replace("webhook_url =", &format!("{bad}\n        webhook_url ="));
            remedy_of(parse(&text).expect_err(bad));
        }
    }

    #[test]
    fn l1_k8_a_zero_or_negative_policy_lease_is_refused() {
        for lease in ["0", "-5"] {
            let text = VALID.replace(
                "webhook_url =",
                &format!("policy = {{ lease_ms = {lease} }}\n        webhook_url ="),
            );
            remedy_of(parse(&text).expect_err("non-positive lease"));
        }
    }

    #[test]
    fn l1_w3_policy_json_renders_integers_strings_and_booleans() {
        let text = VALID.replace(
            "webhook_url =",
            "policy = { max_attempts = 25, ttl_ms = \"never\", flagged = true }\n        \
             webhook_url =",
        );
        let config = parse(&text).expect("valid config");
        let json = Config::policy_json(&config.routes[0]).expect("policy json");
        assert_eq!(
            json,
            "{\"flagged\":true,\"max_attempts\":25,\"ttl_ms\":\"never\"}"
        );
    }

    #[test]
    fn l1_k6_k10_the_shipped_example_config_is_valid() {
        // Security/gap audit: the file the runbook copies to LXC 109
        // must never ship with a typo the validator would refuse.
        let shipped = include_str!("../deploy/config.toml");
        let config = parse(shipped).expect("deploy/config.toml parses");
        assert!(
            config
                .routes
                .iter()
                .any(|route| route.topic == "mailbox.events"),
            "the K6 default route is present"
        );
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
