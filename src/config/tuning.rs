//! The `[defaults]` and `[tuning]` blocks: per-runner values with a
//! built-in default for each, so a config that leaves a block out
//! behaves exactly like one that spells out the defaults.

use std::time::Duration;

use serde::Deserialize;

use super::ConfigError;

/// Accepted range for each half of a backoff pair (base and max), in ms.
const BACKOFF_RANGE_MS: std::ops::RangeInclusive<u64> = 50..=600_000;

/// Accepted range for the single tuning timings, in ms.
const TIMING_RANGE_MS: std::ops::RangeInclusive<u64> = 100..=600_000;

/// MR1 (mini-round, Kenny 2026-08-30): the operationally meaningful
/// timings — the ones an operator might want to change on a running
/// machine without a rebuild. Every default equals the value that was
/// hardcoded before, so an absent `[tuning]` block behaves identically.
/// Numbers that are correctness-coupled stay pinned constants in the
/// parent module, each with the reason it cannot move.
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

    /// Refuse a backoff pair whose halves leave the accepted range or
    /// whose max sits below its base, and any single timing outside its
    /// range, naming the offending field in the error.
    pub(super) fn validate(&self) -> Result<(), ConfigError> {
        validate_pair("hub_backoff", self.hub_backoff_ms, self.hub_backoff_max_ms)?;
        validate_pair(
            "circuit_probe",
            self.circuit_probe_ms,
            self.circuit_probe_max_ms,
        )?;
        for (field, value) in [
            ("circuit_probe_timeout_ms", self.circuit_probe_timeout_ms),
            ("topic_unborn_poll_ms", self.topic_unborn_poll_ms),
            ("route_respawn_ms", self.route_respawn_ms),
            ("settle_timeout_ms", self.settle_timeout_ms),
        ] {
            if !TIMING_RANGE_MS.contains(&value) {
                return Err(ConfigError::Tuning {
                    field: field.into(),
                    problem: format!(
                        "{value} ms is outside {}-{}",
                        TIMING_RANGE_MS.start(),
                        TIMING_RANGE_MS.end()
                    ),
                });
            }
        }
        Ok(())
    }
}

fn validate_pair(name: &str, base: u64, max: u64) -> Result<(), ConfigError> {
    if BACKOFF_RANGE_MS.contains(&base) && BACKOFF_RANGE_MS.contains(&max) && max >= base {
        return Ok(());
    }
    Err(ConfigError::Tuning {
        field: name.into(),
        problem: format!(
            "base {base} ms and max {max} ms must each be {}-{} and the max may not be below \
             the base",
            BACKOFF_RANGE_MS.start(),
            BACKOFF_RANGE_MS.end()
        ),
    })
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
