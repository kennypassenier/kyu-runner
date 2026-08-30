// L3 · hub-down resilience (K7) and graceful shutdown (W1), E2E.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l3_k7_a_hub_outage_is_one_log_line_and_recovery_is_automatic() {
    let mut hub = Hub::start().await;
    if !hub.supports_restart() {
        // Loud, not silent: under docker-only (CI) the restart drill
        // cannot keep the data dir. Recorded in TEST_PLAN.md.
        eprintln!("SKIPPED: hub restart drill needs KYU_BIN (docker keeps no state)");
        return;
    }
    let ha = FakeHa::start();
    let topic = "l3.outage";
    let bridge = Bridge::start(&route_config(
        &hub,
        "hubdown",
        topic,
        &ha.url("/api/webhook/x"),
    ));

    wait_first_poll(&bridge).await;
    publish(&hub, topic, "text/plain", "warmup").await;
    wait_until("the warm-up delivery", Duration::from_secs(30), || {
        ha.hits().len() == 1
    })
    .await;

    hub.stop();
    wait_until(
        "the hub-down transition line",
        Duration::from_secs(30),
        || bridge.log().contains("hub unreachable"),
    )
    .await;

    // S5: one transition line, not a line per failed poll (AR6). The
    // count is on the message phrase — the error text repeats the
    // words "hub unreachable" within the same line. Gap audit #6: also
    // bound the TOTAL lines added during the outage window, so a new
    // repeated line under any other phrase turns this red too.
    let lines_before = bridge.log().lines().count();
    tokio::time::sleep(Duration::from_secs(10)).await;
    let down_lines = bridge.log().matches("reconnecting with backoff").count();
    assert_eq!(down_lines, 1, "transition-only logging during the outage");
    let added = bridge.log().lines().count().saturating_sub(lines_before);
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
        bridge.log().contains("answering normally again"),
        "the recovery transition is logged"
    );
}

#[tokio::test]
async fn l3_w1_sigterm_mid_delivery_finishes_the_message_and_exits_zero() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l3.sigterm";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-bridge", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-bridge", &setup_id).await;
    put_policy(&hub, topic, "ha-bridge", r#"{"lease_ms":12000}"#).await;

    let config = route_config(&hub, "sigterm", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
    );
    ha.set_mode(HaMode::SlowOk(Duration::from_secs(2)));
    let mut bridge = Bridge::start(&config);
    publish(&hub, topic, "text/plain", "in-flight").await;

    wait_until("the delivery to start", Duration::from_secs(30), || {
        ha.hits().iter().any(|hit| hit.body_str() == "in-flight")
    })
    .await;
    bridge.sigterm();

    // AR9: the in-flight delivery (2 s left) plus the ack fit inside
    // the derived grace (webhook timeout 3 s + 5 s margin).
    let status = bridge
        .wait_exit(Duration::from_secs(15))
        .expect("the bridge exits within the grace");
    assert!(status.success(), "clean exit after SIGTERM: {status}");
    let log = bridge.log();
    assert!(
        log.contains("delivered and acked"),
        "the in-flight delivery completed: {log}"
    );
    assert!(log.contains("hub-bridge stopped"), "orderly stop: {log}");

    // The completed delivery was acked — nothing left on the hub.
    assert_eq!(poll_once(&hub, topic, "ha-bridge").await, None);
}

#[tokio::test]
async fn l3_w1_sigint_also_stops_cleanly() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let mut bridge = Bridge::start(&route_config(
        &hub,
        "sigint",
        "l3.sigint",
        &ha.url("/api/webhook/x"),
    ));
    wait_first_poll(&bridge).await;
    bridge.signal("-INT");
    let status = bridge
        .wait_exit(Duration::from_secs(20))
        .expect("the bridge exits on SIGINT");
    assert!(status.success(), "clean exit after SIGINT: {status}");
    assert!(bridge.log().contains("hub-bridge stopped"));
}

#[tokio::test]
async fn l3_ar1_a_panicking_route_is_respawned_and_the_message_survives() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l3.panic";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-bridge", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-bridge", &setup_id).await;
    put_policy(&hub, topic, "ha-bridge", r#"{"lease_ms":12000}"#).await;

    let config = route_config(&hub, "fragile", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
    );
    // The debug-only hook panics the route on its first claimed
    // message; the supervisor must respawn it and the unacked claim
    // must redeliver (AR1 + K5).
    let bridge = Bridge::start_with_env(&config, &[("HUB_BRIDGE_TEST_PANIC_ROUTE", "fragile")]);
    publish(&hub, topic, "text/plain", "survives-the-panic").await;

    wait_until(
        "the supervisor respawn line",
        Duration::from_secs(60),
        || bridge.log().contains("respawning (AR1)"),
    )
    .await;
    wait_until("the post-panic delivery", Duration::from_secs(90), || {
        ha.hits()
            .iter()
            .any(|hit| hit.body_str() == "survives-the-panic")
            && bridge.log().contains("delivered and acked")
    })
    .await;
}
