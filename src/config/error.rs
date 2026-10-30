//! The configuration error type. Every variant's message ends in a
//! `Remedy:` (standing rules 11/12), so an operator reading the startup
//! log knows what to change without opening the source.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
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
