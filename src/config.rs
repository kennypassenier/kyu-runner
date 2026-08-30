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

pub const DEFAULT_CONFIG_PATH: &str = "/etc/kyu-runner/config.toml";

/// The hub caps `wait` at 300 s (kyu K2); staying under it keeps the
/// hub's answer authoritative instead of silently clamped.
const MAX_POLL_WAIT_MS: u64 = 300_000;

/// The hub's default lease when a route sets no policy of its own
/// (kyu `DEFAULT_LEASE_MS`). If kyu ever changes this default,
/// routes without an explicit `policy.lease_ms` get a wrong budget —
/// which is why the K8 remedy pushes toward setting one.
const HUB_DEFAULT_LEASE_MS: u64 = 30_000;

/// Contract-adjacent, therefore pinned rather than configurable
/// (standing rule "operational knobs are configurable"; mini-round MR1,
/// 2026-08-30): the in-process retry ladder exists to fit inside the
/// lease budget (AR5). Exposing base and cap separately invites a
/// combination that silently outlives the claim, which is the exact
/// failure the budget was built to prevent.
pub const DELIVERY_RETRY_BASE: Duration = Duration::from_secs(1);
pub const DELIVERY_RETRY_CAP: Duration = Duration::from_secs(8);

/// Pinned for the same reason: one bounded retry of a settle call, so a
/// hub blink does not cost a gratuitous duplicate. Longer belongs to
/// `settle_timeout_ms`, which IS configurable.
pub const SETTLE_RETRY_PAUSE: Duration = Duration::from_secs(1);

/// Pinned: not a tuning knob but a busy-loop guard. `accept()` on an
/// exhausted fd table returns instantly, so without this pause the
/// health socket would spin a core while the process is already in
/// trouble (security finding F2).
pub const HEALTH_ACCEPT_PAUSE: Duration = Duration::from_millis(100);

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error(
        "cannot read config file {path}: {source}. Remedy: check that the file exists and is \
         readable; the default location is /etc/kyu-runner/config.toml and --config <path> \
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
        "{field} is {url:?}, which does not start with http:// . Remedy: the runner speaks plain \
         HTTP on the LAN only (AR11); use an http:// address such as http://127.0.0.1:8080. If \
         this URL sits behind a TLS-terminating Traefik, that is the AR11-TLS mini-round: rustls \
         goes in the moment a concrete https target exists."
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

    #[error(
        "[tuning] {field}: {problem}. Remedy: these are the operational timings; leave the whole \
         [tuning] block out to use the built-in defaults, or pick a value inside the stated range."
    )]
    Tuning { field: String, problem: String },

    #[error("route {name:?}: {problem} Remedy: {remedy}")]
    Route {
        name: String,
        problem: String,
        remedy: String,
    },

    #[error(
        "{field} is {url:?}, which is not a usable http URL ({reason}). Remedy: use a full \
         http://host[:port]/path address without credentials — the runner sends its token in a \
         header, never in a URL."
    )]
    UrlInvalid {
        field: String,
        url: String,
        reason: String,
    },

    #[error(
        "[defaults] max_body_bytes is {value}, below 1024. Remedy: the cap protects the runner \
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
    pub tuning: Tuning,
    #[serde(default)]
    pub routes: Vec<Route>,
}

/// MR1 (mini-round, Kenny 2026-08-30): the operationally meaningful
/// timings — the ones an operator might want to change on a running
/// machine without a rebuild. Every default equals the value that was
/// hardcoded before, so an absent `[tuning]` block behaves identically.
/// Numbers that are correctness-coupled stay pinned constants above,
/// each with the reason it cannot move.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Tuning {
    /// First and longest pause between reconnect attempts while the hub
    /// is unreachable (K7/AR6).
    pub hub_backoff_ms: u64,
    pub hub_backoff_max_ms: u64,
    /// First and longest pause between probes while a route's circuit
    /// is open because the webhook target is down (AR15).
    pub circuit_probe_ms: u64,
    pub circuit_probe_max_ms: u64,
    /// How long a single TCP probe of the webhook host may take.
    pub circuit_probe_timeout_ms: u64,
    /// How often to re-poll a topic that does not exist yet (AR16).
    pub topic_unborn_poll_ms: u64,
    /// Pause before a panicked route loop is respawned (AR1).
    pub route_respawn_ms: u64,
    /// Timeout on ack/nack/policy calls; also the margin reserved
    /// inside the lease budget for settling (AR5), so raising it
    /// tightens the budget instead of silently breaking the invariant.
    pub settle_timeout_ms: u64,
    /// Health socket limits (W4/AR10, security finding F2).
    pub healthz_max_connections: usize,
    pub healthz_timeout_ms: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            hub_backoff_ms: 500,
            hub_backoff_max_ms: 30_000,
            circuit_probe_ms: 1_000,
            circuit_probe_max_ms: 60_000,
            circuit_probe_timeout_ms: 3_000,
            topic_unborn_poll_ms: 5_000,
            route_respawn_ms: 5_000,
            settle_timeout_ms: 5_000,
            healthz_max_connections: 16,
            healthz_timeout_ms: 5_000,
        }
    }
}

impl Tuning {
    pub fn hub_backoff(&self) -> (u64, u64) {
        (self.hub_backoff_ms, self.hub_backoff_max_ms)
    }

    pub fn circuit_backoff(&self) -> (u64, u64) {
        (self.circuit_probe_ms, self.circuit_probe_max_ms)
    }

    pub fn circuit_probe_timeout(&self) -> Duration {
        Duration::from_millis(self.circuit_probe_timeout_ms)
    }

    pub fn topic_unborn_poll(&self) -> Duration {
        Duration::from_millis(self.topic_unborn_poll_ms)
    }

    pub fn route_respawn(&self) -> Duration {
        Duration::from_millis(self.route_respawn_ms)
    }

    pub fn settle_timeout(&self) -> Duration {
        Duration::from_millis(self.settle_timeout_ms)
    }

    pub fn healthz_timeout(&self) -> Duration {
        Duration::from_millis(self.healthz_timeout_ms)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let pair = |name: &str, base: u64, max: u64| -> Result<(), ConfigError> {
            if !(50..=600_000).contains(&base) || !(50..=600_000).contains(&max) || max < base {
                return Err(ConfigError::Tuning {
                    field: name.into(),
                    problem: format!(
                        "base {base} ms and max {max} ms must each be 50-600000 and the max may \
                         not be below the base"
                    ),
                });
            }
            Ok(())
        };
        pair("hub_backoff", self.hub_backoff_ms, self.hub_backoff_max_ms)?;
        pair(
            "circuit_probe",
            self.circuit_probe_ms,
            self.circuit_probe_max_ms,
        )?;
        for (field, value) in [
            ("circuit_probe_timeout_ms", self.circuit_probe_timeout_ms),
            ("topic_unborn_poll_ms", self.topic_unborn_poll_ms),
            ("route_respawn_ms", self.route_respawn_ms),
            ("settle_timeout_ms", self.settle_timeout_ms),
            ("healthz_timeout_ms", self.healthz_timeout_ms),
        ] {
            if !(100..=600_000).contains(&value) {
                return Err(ConfigError::Tuning {
                    field: field.into(),
                    problem: format!("{value} ms is outside 100-600000"),
                });
            }
        }
        if !(1..=1_000).contains(&self.healthz_max_connections) {
            return Err(ConfigError::Tuning {
                field: "healthz_max_connections".into(),
                problem: format!("{} is outside 1-1000", self.healthz_max_connections),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    pub poll_wait_ms: u64,
    pub webhook_timeout_ms: u64,
    /// Security F1: a claimed message is buffered in memory before the
    /// webhook POST; this caps it so a misbehaving hub cannot OOM the
    /// runner. An oversize message is nacked and dead-letters visibly.
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
    /// The runner does not interpret it — the hub validates policies and
    /// refuses unusable ones with a remedy (kyu K7), so hardcoding
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
        Duration::from_millis(max_timeout + self.tuning.settle_timeout_ms)
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
    /// runner does not know the hub's field names on purpose (the hub
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
        self.tuning.validate()?;
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
            if timeout + self.tuning.settle_timeout_ms > budget {
                return Err(ConfigError::LeaseBudget {
                    name: route.name.clone(),
                    timeout,
                    margin: self.tuning.settle_timeout_ms,
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
                 e.g. \"kyu-events\".",
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
                "subscription names are kyu's one concept; keep them plain, \
                 e.g. \"ha-runner\".",
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
        name = "kyu-events"
        topic = "kyu.events"
        subscription = "ha-runner"
        webhook_url = "http://ha.lan:8123/api/webhook/hub_kyu_events"
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
            "{VALID}\n[[routes]]\nname = \"kyu-events\"\ntopic = \"t2\"\n\
             subscription = \"s2\"\nwebhook_url = \"http://ha.lan:8123/api/webhook/x\"\n"
        );
        let message = remedy_of(parse(&text).expect_err("dup name"));
        assert!(message.contains("used by another route"), "{message}");
    }

    #[test]
    fn l1_k8_a_duplicate_topic_subscription_pair_is_refused() {
        let text = format!(
            "{VALID}\n[[routes]]\nname = \"second\"\ntopic = \"kyu.events\"\n\
             subscription = \"ha-runner\"\nwebhook_url = \"http://ha.lan:8123/api/webhook/x\"\n"
        );
        let message = remedy_of(parse(&text).expect_err("dup pair"));
        assert!(message.contains("compete"), "{message}");
    }

    #[test]
    fn l1_k8_bad_identifier_characters_are_refused() {
        for (field, bad) in [
            ("name = \"kyu-events\"", "name = \"mail box\""),
            ("topic = \"kyu.events\"", "topic = \"kyu/events\""),
            ("subscription = \"ha-runner\"", "subscription = \"\""),
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
            "webhook_url = \"http://ha.lan:8123/api/webhook/hub_kyu_events\"",
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
        assert!(message.contains("kyu-events"), "{message}");
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
                .any(|route| route.topic == "kyu.events"),
            "the K6 default route is present"
        );
    }

    #[test]
    fn l7_mr1_the_tuning_defaults_match_the_previously_hardcoded_values() {
        let config = parse(VALID).expect("valid config");
        assert_eq!(config.tuning.hub_backoff(), (500, 30_000));
        assert_eq!(config.tuning.circuit_backoff(), (1_000, 60_000));
        assert_eq!(
            config.tuning.circuit_probe_timeout(),
            Duration::from_secs(3)
        );
        assert_eq!(config.tuning.topic_unborn_poll(), Duration::from_secs(5));
        assert_eq!(config.tuning.route_respawn(), Duration::from_secs(5));
        assert_eq!(config.tuning.settle_timeout(), Duration::from_secs(5));
        assert_eq!(config.tuning.healthz_max_connections, 16);
        assert_eq!(config.tuning.healthz_timeout(), Duration::from_secs(5));
    }

    #[test]
    fn l7_mr1_a_tuning_block_overrides_only_what_it_names() {
        let text = format!("{VALID}\n[tuning]\nhub_backoff_max_ms = 90000\n");
        let config = parse(&text).expect("valid config");
        assert_eq!(config.tuning.hub_backoff(), (500, 90_000));
        assert_eq!(config.tuning.route_respawn(), Duration::from_secs(5));
    }

    #[test]
    fn l7_mr1_an_inverted_backoff_pair_is_refused_with_a_remedy() {
        let text = format!("{VALID}\n[tuning]\nhub_backoff_ms = 30000\nhub_backoff_max_ms = 500\n");
        let message = remedy_of(parse(&text).expect_err("max below base"));
        assert!(message.contains("hub_backoff"), "{message}");
    }

    #[test]
    fn l7_mr1_out_of_range_tuning_values_are_refused() {
        for line in [
            "settle_timeout_ms = 10",
            "topic_unborn_poll_ms = 900000",
            "healthz_max_connections = 0",
            "circuit_probe_timeout_ms = 50",
        ] {
            let text = format!("{VALID}\n[tuning]\n{line}\n");
            remedy_of(parse(&text).expect_err(line));
        }
    }

    #[test]
    fn l7_mr1_an_unknown_tuning_key_is_refused() {
        let text = format!("{VALID}\n[tuning]\nhub_backoff_millis = 500\n");
        remedy_of(parse(&text).expect_err("typo in a tuning key"));
    }

    #[test]
    fn l7_mr1_a_raised_settle_timeout_tightens_the_lease_budget() {
        // AR5's invariant must follow the configured settle timeout
        // instead of drifting from it: with settle 10 s the 17 s
        // webhook timeout no longer fits the 21 s budget.
        let text = VALID.replace(
            "webhook_url =",
            "webhook_timeout_ms = 15000\n        webhook_url =",
        );
        parse(&text).expect("15s + 5s fits the default 21s budget");
        let tightened = format!("{text}\n[tuning]\nsettle_timeout_ms = 10000\n");
        let message = remedy_of(parse(&tightened).expect_err("15s + 10s does not fit"));
        assert!(message.contains("settle margin"), "{message}");
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
