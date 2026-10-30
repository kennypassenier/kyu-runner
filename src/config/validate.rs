//! Fail-closed validation of a parsed `Config` (K8). Each check returns
//! the first problem it finds as a `ConfigError` with a remedy; nothing
//! here is corrected silently except the trailing-slash normalisation
//! of `hub_url`, which changes no meaning.

use std::collections::HashSet;

use super::{Config, ConfigError, Route};

/// The hub caps `wait` at 300 s (kyu K2); staying under it keeps the
/// hub's answer authoritative instead of silently clamped.
const MAX_POLL_WAIT_MS: u64 = 300_000;

/// Below one second a long-poll is a busy loop against the hub.
const MIN_POLL_WAIT_MS: u64 = 1_000;

/// The shortest webhook timeout that still gives HA a fair chance.
const MIN_WEBHOOK_TIMEOUT_MS: u64 = 100;

/// The smallest body cap; below this even an ordinary event is refused.
const MIN_BODY_BYTES: u64 = 1024;

/// The lease a route may ask the hub for: one second to one day.
const POLICY_LEASE_RANGE_MS: std::ops::RangeInclusive<i64> = 1_000..=86_400_000;

/// Security F6: a prefix check alone let `http://` through, which then
/// wedged a route in circuit-open forever at runtime. Parse for real.
pub(super) fn validate_url(field: &str, url: &str) -> Result<(), ConfigError> {
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

impl Config {
    pub(super) fn validate(&mut self) -> Result<(), ConfigError> {
        self.tuning.validate()?;
        while self.hub_url.ends_with('/') {
            self.hub_url.pop();
        }
        validate_url("hub_url", &self.hub_url)?;
        self.validate_defaults()?;
        if self.routes.is_empty() {
            return Err(ConfigError::NoRoutes);
        }
        let mut names = HashSet::new();
        let mut pairs = HashSet::new();
        for route in &self.routes {
            route.validate()?;
            validate_route_timeout(route)?;
            validate_policy(route)?;
            self.validate_lease_budget(route)?;
            check_unique(route, &mut names, &mut pairs)?;
        }
        Ok(())
    }

    fn validate_defaults(&self) -> Result<(), ConfigError> {
        let defaults = &self.defaults;
        if !(MIN_POLL_WAIT_MS..=MAX_POLL_WAIT_MS).contains(&defaults.poll_wait_ms) {
            return Err(ConfigError::PollWait {
                value: defaults.poll_wait_ms,
            });
        }
        if defaults.webhook_timeout_ms < MIN_WEBHOOK_TIMEOUT_MS {
            return Err(ConfigError::WebhookTimeout {
                scope: "[defaults]".into(),
                value: defaults.webhook_timeout_ms,
            });
        }
        if defaults.max_body_bytes < MIN_BODY_BYTES {
            return Err(ConfigError::BodyCap {
                value: defaults.max_body_bytes,
            });
        }
        Ok(())
    }

    /// AR5: the webhook timeout plus the settle margin must fit the
    /// lease budget, so a delivery is always settled by its owner.
    fn validate_lease_budget(&self, route: &Route) -> Result<(), ConfigError> {
        let timeout = route
            .webhook_timeout_ms
            .unwrap_or(self.defaults.webhook_timeout_ms);
        let lease = Self::effective_lease_ms(route);
        let budget = super::budget_of(lease);
        if timeout + self.tuning.settle_timeout_ms <= budget {
            return Ok(());
        }
        Err(ConfigError::LeaseBudget {
            name: route.name.clone(),
            timeout,
            margin: self.tuning.settle_timeout_ms,
            budget,
            lease,
        })
    }
}

fn validate_route_timeout(route: &Route) -> Result<(), ConfigError> {
    match route.webhook_timeout_ms {
        Some(value) if value < MIN_WEBHOOK_TIMEOUT_MS => Err(ConfigError::WebhookTimeout {
            scope: format!("route {:?}", route.name),
            value,
        }),
        _ => Ok(()),
    }
}

/// W3 policy blocks are flat hub fields. Keys must be plain field names
/// and values must survive the hand-built JSON (Security F7), and a
/// `lease_ms` must be a whole number of milliseconds in range.
fn validate_policy(route: &Route) -> Result<(), ConfigError> {
    let Some(policy) = &route.policy else {
        return Ok(());
    };
    for (key, value) in policy {
        validate_policy_key(route, key)?;
        validate_policy_value(route, key, value)?;
    }
    if let Some(lease) = policy.get("lease_ms")
        && !lease
            .as_integer()
            .is_some_and(|value| POLICY_LEASE_RANGE_MS.contains(&value))
    {
        return Err(ConfigError::PolicyLease {
            name: route.name.clone(),
            value: lease.to_string(),
        });
    }
    Ok(())
}

fn validate_policy_key(route: &Route, key: &str) -> Result<(), ConfigError> {
    if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Ok(());
    }
    Err(route.error(
        format!("policy key {key:?} is not a plain field name."),
        "policy keys are the hub's snake_case fields, e.g. lease_ms, max_attempts.",
    ))
}

/// Security F7: the policy JSON is hand-built, so values that would need
/// JSON escaping are refused rather than mis-encoded (which would make
/// the PUT fail forever at runtime).
fn validate_policy_value(route: &Route, key: &str, value: &toml::Value) -> Result<(), ConfigError> {
    let clean = match value {
        toml::Value::Integer(_) | toml::Value::Boolean(_) => true,
        toml::Value::String(text) => text
            .bytes()
            .all(|b| (0x20..=0x7e).contains(&b) && b != b'"' && b != b'\\'),
        _ => false,
    };
    if clean {
        return Ok(());
    }
    Err(route.error(
        format!("policy.{key} is {value}, which is not an integer, boolean or plain ASCII string."),
        "policy blocks are flat hub fields, e.g. lease_ms = 300000, max_attempts = 25, \
         ttl_ms = \"never\" — see the hub's policy endpoint for what it accepts.",
    ))
}

/// Two routes may share neither a name (the log/health handle) nor a
/// topic+subscription pair (they would split each other's messages).
fn check_unique(
    route: &Route,
    names: &mut HashSet<String>,
    pairs: &mut HashSet<(String, String)>,
) -> Result<(), ConfigError> {
    if !names.insert(route.name.clone()) {
        return Err(route.error(
            "its name is used by another route.",
            "route names are the log and health handles, so give every [[routes]] block its \
             own.",
        ));
    }
    if !pairs.insert((route.topic.clone(), route.subscription.clone())) {
        return Err(route.error(
            format!(
                "another route already consumes topic {:?} as subscription {:?} — the two \
                 would compete for the same messages and split them between webhooks.",
                route.topic, route.subscription
            ),
            "give this route its own subscription name (fan-out: each name receives every \
             message) or remove the duplicate.",
        ));
    }
    Ok(())
}
