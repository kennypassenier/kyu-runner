//! Configuration: parse and validate, fail-closed (K8). Unknown keys,
//! duplicate routes and unusable values are startup errors, each with a
//! remedy in the message (standing rules 11/12). Pure — the only I/O is
//! the file read in `load`.
//!
//! Layout: `error` holds the error type, `tuning` the `[defaults]` and
//! `[tuning]` blocks, `route` one `[[routes]]` block, and `validate` the
//! checks that tie them together. This file keeps the top-level shape and
//! the values derived from it.

mod error;
mod route;
mod tuning;
mod validate;

use std::time::Duration;

use serde::Deserialize;

pub use error::ConfigError;
pub use route::Route;
pub use tuning::{Defaults, Tuning};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/kyu-runner/config.toml";

/// The hub's default lease when a route sets no policy of its own
/// (kyu `DEFAULT_LEASE_MS`). If kyu ever changes this default,
/// routes without an explicit `policy.lease_ms` get a wrong budget —
/// which is why the K8 remedy pushes toward setting one.
const HUB_DEFAULT_LEASE_MS: u64 = 30_000;

/// AR5: in-process retries may use this share of the lease (in percent),
/// leaving the rest as headroom so the claim never expires mid-delivery.
const LEASE_BUDGET_PERCENT: u64 = 70;

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

/// The lease budget in ms for a lease of `lease_ms`.
fn budget_of(lease_ms: u64) -> u64 {
    lease_ms * LEASE_BUDGET_PERCENT / 100
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub hub_url: String,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub tuning: Tuning,
    #[serde(default)]
    pub routes: Vec<Route>,
}

impl Config {
    pub fn from_project_table(table: &toml::Table) -> Result<Config, ConfigError> {
        from_project_table(table)
    }

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
        Duration::from_millis(budget_of(Self::effective_lease_ms(route)))
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
}

/// Build the pump's config from the project's own half of the shared
/// config file, so `deny_unknown_fields` keeps refusing typos in OUR keys
/// while the two configurations live in one file.
///
/// The kit strips its own half since chassis 2.0.0 (`App::project_table`,
/// feat-config-1). Before that this function did it here, and the line it
/// needed for the kit's `[[notify.webhook]]` tables — `own.remove("notify")`
/// — rested on knowledge nothing documented: it would have gone on
/// compiling and started refusing valid configs the day the kit grew a
/// second section of its own.
pub fn from_project_table(table: &toml::Table) -> Result<Config, ConfigError> {
    let mut config: Config = toml::Value::Table(table.clone())
        .try_into()
        .map_err(Box::new)?;
    config.validate()?;
    Ok(config)
}

pub fn parse(text: &str) -> Result<Config, ConfigError> {
    let mut config: Config = toml::from_str(text).map_err(Box::new)?;
    config.validate()?;
    Ok(config)
}

#[cfg(test)]
mod test_fixtures;
#[cfg(test)]
mod tests_parse;
#[cfg(test)]
mod tests_policy;
