//! Parsing and the basic K8 refusals: shape, URLs, identifiers,
//! uniqueness and the poll/timeout bounds.

use super::test_fixtures::{VALID, remedy_of};
use super::*;

#[test]
fn l1_k8_a_valid_config_parses_and_normalizes() {
    let config = parse(VALID).expect("valid config");
    assert_eq!(config.hub_url, "http://127.0.0.1:8080");
    assert_eq!(config.defaults.poll_wait_ms, 25_000);
    assert_eq!(
        config.webhook_timeout(&config.routes[0]),
        Duration::from_millis(10_000)
    );
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
