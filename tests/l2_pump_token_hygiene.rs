// L2 · the pump (K1-K5) end-to-end: a real kyu hub (scratch), a
// fake HA webhook server, the runner as a real process. Scenario names
// carry the scope's S-ids.
// This part: the hub token never reaches the logs, and a missing token
// is reported once.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l2_k9_ar7_the_token_reaches_the_hub_but_never_the_logs() {
    let hub = Hub::start().await;
    // Since 3.0.0 the hub issues this token itself; the claim is
    // unchanged — whatever the runner sends must never reach a log line.
    let token = hub.client_token().to_string();
    let ha = FakeHa::start();
    let topic = "l2.door";
    let sentinel = "privacy-sentinel-payload";

    let config = route_config(&hub, "door", topic, &ha.url("/api/webhook/x"));
    let runner = Runner::start_with_env(&hub, &config, &[("KYU_RUNNER_TOKEN", &token)]);
    wait_first_poll(&runner).await;
    publish_authed(&hub, Some(&token), topic, "text/plain", sentinel).await;

    wait_until("the doored delivery", Duration::from_secs(30), || {
        ha.hits().iter().any(|hit| hit.body_str() == sentinel)
    })
    .await;
    let log = runner.log();
    assert!(
        !log.contains(&token),
        "the token must never reach a log line (rule 10/AR7)"
    );
    assert!(
        !log.contains(sentinel),
        "payloads must never reach a log line (AR14)"
    );
}

#[tokio::test]
async fn l2_k9_a_missing_token_logs_the_apps_remedy_once_without_flooding() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l2.locked";

    // The runner is started with NO token against a hub whose door is
    // always on: the 401 path, which is what this test is about.
    let runner = Runner::start_with_env(
        &hub,
        &route_config(&hub, "locked", topic, &ha.url("/api/webhook/x")),
        &[("KYU_RUNNER_TOKEN", NO_HUB_TOKEN)],
    );
    publish(&hub, topic, "text/plain", "waits").await;

    wait_until("the 401 remedy", Duration::from_secs(30), || {
        runner.log().contains("/apps")
    })
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let mentions = runner.log().matches("/apps").count();
    assert!(
        mentions <= 2,
        "auth denial joins transition-only logging (AR6): {mentions} mentions"
    );
    assert!(
        ha.hits().is_empty(),
        "nothing may be delivered without auth"
    );
}

#[tokio::test]
async fn l2_k9_the_token_stays_out_of_failure_path_and_json_logs() {
    let mut hub = Hub::start().await;
    let token = hub.client_token().to_string();
    let ha = FakeHa::start();
    let topic = "l2.doorfail";
    let sentinel = "faalpad-sentinel-payload";

    // JSON log format (Loki) with everything at trace level — the
    // widest possible net for a leak (gap audit #5).
    let config = route_config(&hub, "doorfail", topic, &ha.url("/api/webhook/x"));
    let runner = Runner::start_with_env(
        &hub,
        &config,
        &[
            ("KYU_RUNNER_TOKEN", &token),
            ("KYU_RUNNER_LOG", "trace"),
            ("KYU_RUNNER_LOG_FORMAT", "json"),
        ],
    );
    wait_first_poll(&runner).await;
    publish_authed(&hub, Some(&token), topic, "text/plain", sentinel).await;
    wait_until("the delivery", Duration::from_secs(30), || {
        ha.hits().iter().any(|hit| hit.body_str() == sentinel)
    })
    .await;

    // Now the failure paths: hub gone mid-run.
    hub.stop();
    wait_until("the hub-down line", Duration::from_secs(30), || {
        runner.log().contains("hub unreachable")
    })
    .await;

    let log = runner.log();
    assert!(log.contains("{\""), "json log format is in effect");
    assert!(
        !log.contains(&token),
        "the token must survive trace-level failure paths unlogged (rule 10)"
    );
    assert!(
        !log.contains(sentinel),
        "payloads stay out at trace level too"
    );
}
