//! Lease budget (AR5), shutdown grace (AR9), policy blocks (W3), the
//! shipped example config and the `[tuning]` block (MR1).

use super::test_fixtures::{VALID, remedy_of};
use super::*;

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
    let shipped = include_str!("../../deploy/config.toml");
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
