// L3 · hub-down resilience (K7) and graceful shutdown (W1), E2E.
// This part: signals and the bounded shutdown.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l3_w1_sigterm_mid_delivery_finishes_the_message_and_exits_zero() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l3.sigterm";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;
    put_policy(&hub, topic, "ha-runner", r#"{"lease_ms":12000}"#).await;

    let config = route_config(&hub, "sigterm", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
    );
    ha.set_mode(HaMode::SlowOk(Duration::from_secs(2)));
    let mut runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", "in-flight").await;

    wait_until("the delivery to start", Duration::from_secs(30), || {
        ha.hits().iter().any(|hit| hit.body_str() == "in-flight")
    })
    .await;
    runner.sigterm();

    // AR9: the in-flight delivery (2 s left) plus the ack fit inside
    // the derived grace (webhook timeout 3 s + 5 s margin).
    let status = runner
        .wait_exit(Duration::from_secs(15))
        .expect("the runner exits within the grace");
    assert!(status.success(), "clean exit after SIGTERM: {status}");
    let log = runner.log();
    assert!(
        log.contains("delivered and acked"),
        "the in-flight delivery completed: {log}"
    );
    assert!(log.contains("kyu-runner stopped"), "orderly stop: {log}");

    // The completed delivery was acked — nothing left on the hub.
    assert_eq!(poll_once(&hub, topic, "ha-runner").await, None);
}

#[tokio::test]
async fn l3_w1_sigint_also_stops_cleanly() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let mut runner = Runner::start(
        &hub,
        &route_config(&hub, "sigint", "l3.sigint", &ha.url("/api/webhook/x")),
    );
    wait_first_poll(&runner).await;
    runner.signal("-INT");
    let status = runner
        .wait_exit(Duration::from_secs(20))
        .expect("the runner exits on SIGINT");
    assert!(status.success(), "clean exit after SIGINT: {status}");
    assert!(runner.log().contains("kyu-runner stopped"));
}

#[tokio::test]
async fn l3_w1_a_second_signal_is_harmless_and_the_stop_still_finishes() {
    // G3 (closed at Kenny's ratification 2026-08-30) asked for exit 130 on
    // a second signal. Since the chassis migration the kit owns the
    // signals: a second SIGTERM is ignored by design (systemd's SIGKILL
    // after TimeoutStopSec is the backstop), so the in-flight delivery
    // finishes and the stop exits 0 inside the bound.
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l3.twosignals";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;
    put_policy(&hub, topic, "ha-runner", r#"{"lease_ms":30000}"#).await;

    let config = route_config(&hub, "twosignals", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 15000\npolicy = { lease_ms = 30000 }\nwebhook_url =",
    );
    // A delivery that would occupy the whole grace if left alone.
    ha.set_mode(HaMode::SlowOk(Duration::from_secs(12)));
    let mut runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", "in-flight").await;
    wait_until(
        "the slow delivery to start",
        Duration::from_secs(30),
        || ha.hits().iter().any(|hit| hit.body_str() == "in-flight"),
    )
    .await;

    let asked = std::time::Instant::now();
    runner.sigterm();
    tokio::time::sleep(Duration::from_millis(300)).await;
    runner.sigterm();

    let status = runner
        .wait_exit(Duration::from_secs(25))
        .expect("the stop finishes inside the kit's bound");
    assert!(
        status.success(),
        "a second signal changes nothing: {status}"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(25),
        "the stop took {:?}",
        asked.elapsed()
    );
}

#[tokio::test]
async fn l3_ar9_shutdown_never_outlasts_the_derived_grace() {
    // G3, second half: whatever a route is doing, systemd's stop must
    // finish inside the window the unit was given. The abort branch
    // behind it is a backstop a cooperating loop never reaches — see
    // TEST_PLAN.
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l3.grace";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;
    put_policy(&hub, topic, "ha-runner", r#"{"lease_ms":30000}"#).await;

    // Grace = webhook timeout (8 s) + settle margin (5 s) = 13 s.
    let config = route_config(&hub, "grace", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 8000\npolicy = { lease_ms = 30000 }\nwebhook_url =",
    );
    ha.set_mode(HaMode::SlowOk(Duration::from_secs(30)));
    let mut runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", "stuck").await;
    wait_until(
        "the stuck delivery to start",
        Duration::from_secs(30),
        || ha.hits().iter().any(|hit| hit.body_str() == "stuck"),
    )
    .await;

    let asked = std::time::Instant::now();
    runner.sigterm();
    let status = runner
        .wait_exit(Duration::from_secs(25))
        .expect("the runner must exit, stuck delivery or not");
    let took = asked.elapsed();
    assert!(
        took < Duration::from_secs(22),
        "shutdown took {took:?}, beyond the 13 s grace plus the kit's slack"
    );
    assert!(
        status.success(),
        "an orderly stop reports 0 (norm N1): {status}"
    );
}
