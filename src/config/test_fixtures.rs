//! Shared fixtures for the config unit tests.

use super::ConfigError;

/// The smallest valid config: one route, every default left implicit.
pub(super) const VALID: &str = r#"
    hub_url = "http://127.0.0.1:8080/"

    [[routes]]
    name = "kyu-events"
    topic = "kyu.events"
    subscription = "ha-runner"
    webhook_url = "http://ha.lan:8123/api/webhook/hub_kyu_events"
"#;

/// The error's text, after asserting it carries a remedy (rule 11).
pub(super) fn remedy_of(error: ConfigError) -> String {
    let text = error.to_string();
    assert!(
        text.contains("Remedy:"),
        "K8/rule 11: every config error carries a remedy — got: {text}"
    );
    text
}
