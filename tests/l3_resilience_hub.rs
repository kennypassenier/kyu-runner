// L3 · hub-down resilience (K7) and graceful shutdown (W1), E2E.
// This part: hub outage, route respawn and token recovery.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l3_k7_a_hub_outage_is_one_log_line_and_recovery_is_automatic() {
    let mut hub = Hub::start().await;
    if !hub.supports_restart() {
        // Loud, not silent: under docker-only (no KYU_BIN) the restart drill
        // cannot keep the data dir. Recorded in TEST_PLAN.md.
        eprintln!("SKIPPED: hub restart drill needs KYU_BIN (docker keeps no state)");
        return;
    }
    let ha = FakeHa::start();
    let topic = "l3.outage";
    let runner = Runner::start(
        &hub,
        &route_config(&hub, "hubdown", topic, &ha.url("/api/webhook/x")),
    );

    wait_first_poll(&runner).await;
    publish(&hub, topic, "text/plain", "warmup").await;
    wait_until("the warm-up delivery", Duration::from_secs(30), || {
        ha.hits().len() == 1
    })
    .await;

    hub.stop();
    wait_until(
        "the hub-down transition line",
        Duration::from_secs(30),
        || runner.log().contains("hub unreachable"),
    )
    .await;

    // S5: one transition line, not a line per failed poll (AR6). The
    // count is on the message phrase — the error text repeats the
    // words "hub unreachable" within the same line. Gap audit #6: also
    // bound the TOTAL lines added during the outage window, so a new
    // repeated line under any other phrase turns this red too.
    let lines_before = runner.log().lines().count();
    tokio::time::sleep(Duration::from_secs(10)).await;
    let down_lines = runner.log().matches("reconnecting with backoff").count();
    assert_eq!(down_lines, 1, "transition-only logging during the outage");
    let added = runner.log().lines().count().saturating_sub(lines_before);
    assert!(
        added <= 3,
        "log volume during a 10 s outage must be a handful of lines, got {added}"
    );

    hub.restart().await;
    publish(&hub, topic, "text/plain", "after-outage").await;
    wait_until(
        "delivery after the hub returned",
        Duration::from_secs(90),
        || ha.hits().iter().any(|hit| hit.body_str() == "after-outage"),
    )
    .await;
    assert!(
        runner.log().contains("answering normally again"),
        "the recovery transition is logged"
    );
}

#[tokio::test]
async fn l3_ar1_a_panicking_route_is_respawned_and_the_message_survives() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l3.panic";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;
    put_policy(&hub, topic, "ha-runner", r#"{"lease_ms":12000}"#).await;

    let config = route_config(&hub, "fragile", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
    );
    // The debug-only hook panics the route on its first claimed
    // message; the supervisor must respawn it and the unacked claim
    // must redeliver (AR1 + K5).
    let runner =
        Runner::start_with_env(&hub, &config, &[("KYU_RUNNER_TEST_PANIC_ROUTE", "fragile")]);
    publish(&hub, topic, "text/plain", "survives-the-panic").await;

    wait_until(
        "the supervisor respawn line",
        Duration::from_secs(60),
        || runner.log().contains("respawning (AR1)"),
    )
    .await;
    wait_until("the post-panic delivery", Duration::from_secs(90), || {
        ha.hits()
            .iter()
            .any(|hit| hit.body_str() == "survives-the-panic")
            && runner.log().contains("delivered and acked")
    })
    .await;
}

#[tokio::test]
async fn l3_k9_a_route_resumes_once_the_hub_accepts_its_token_again() {
    // G5 (closed at Kenny's ratification): a 401 is not a dead end.
    // Scenario: the hub's door changes under a running runner — the
    // runner keeps its token, backs off, and must pick up by itself
    // once the hub accepts that token again, with no restart.
    let mut hub = Hub::start().await;
    if !hub.supports_restart() {
        eprintln!("SKIPPED: the token drill needs KYU_BIN (docker keeps no state)");
        return;
    }
    let ha = FakeHa::start();
    let topic = "l3.token";
    // The runner keeps THIS token for the whole test; only the hub's
    // admission moves. Since 3.0.0 that means swapping the store the
    // door consults, because a client token cannot be dictated.
    let runner_token = hub.client_token().to_string();

    let config = route_config(&hub, "doorman", topic, &ha.url("/api/webhook/x"));
    hub.stop();
    let known_clients = hub.restart_without_clients().await;
    let runner = Runner::start_with_env(&hub, &config, &[("KYU_RUNNER_TOKEN", &runner_token)]);

    wait_until("the 401 remedy", Duration::from_secs(30), || {
        runner.log().contains("/apps")
    })
    .await;
    assert!(ha.hits().is_empty(), "nothing is delivered while denied");

    // The hub knows the runner's token again — the runner is untouched.
    hub.stop();
    hub.restore_clients(known_clients).await;

    // Only publish once the runner is polling again: a subscription
    // created on an ALREADY existing topic starts from now (AR16), so
    // publishing first would hide the very delivery under test.
    wait_until("the runner to poll again", Duration::from_secs(60), || {
        runner.log().contains("topic not born yet")
    })
    .await;
    publish_authed(&hub, Some(&runner_token), topic, "text/plain", "na-de-deur").await;

    wait_until(
        "delivery after the door reopened",
        Duration::from_secs(90),
        || ha.hits().iter().any(|hit| hit.body_str() == "na-de-deur"),
    )
    .await;
    assert!(
        runner.log().contains("answering normally again"),
        "the recovery transition is logged: {}",
        runner.log()
    );
}
