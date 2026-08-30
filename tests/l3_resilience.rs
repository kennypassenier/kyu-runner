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
    let runner = Runner::start(&route_config(
        &hub,
        "hubdown",
        topic,
        &ha.url("/api/webhook/x"),
    ));

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
    let mut runner = Runner::start(&config);
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
    let mut runner = Runner::start(&route_config(
        &hub,
        "sigint",
        "l3.sigint",
        &ha.url("/api/webhook/x"),
    ));
    wait_first_poll(&runner).await;
    runner.signal("-INT");
    let status = runner
        .wait_exit(Duration::from_secs(20))
        .expect("the runner exits on SIGINT");
    assert!(status.success(), "clean exit after SIGINT: {status}");
    assert!(runner.log().contains("kyu-runner stopped"));
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
    let runner = Runner::start_with_env(&config, &[("KYU_RUNNER_TEST_PANIC_ROUTE", "fragile")]);
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
async fn l3_w1_a_second_signal_exits_immediately() {
    // G3 (closed at Kenny's ratification 2026-08-30): the impatient
    // path. systemd sends a second signal when a stop takes too long,
    // and the runner must then leave at once — safe, because an
    // unacked claim simply redelivers (K5).
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
    let mut runner = Runner::start(&config);
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
        .wait_exit(Duration::from_secs(10))
        .expect("the second signal must not wait for the grace");
    assert_eq!(
        status.code(),
        Some(130),
        "an interrupted stop reports 130, the shell's convention"
    );
    assert!(
        asked.elapsed() < Duration::from_secs(10),
        "the second signal took {:?}, which is not 'immediately'",
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
    let mut runner = Runner::start(&config);
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
        took < Duration::from_secs(20),
        "shutdown took {took:?}, beyond the 13 s grace plus slack"
    );
    assert!(
        status.success() || status.code() == Some(130),
        "an orderly stop reports 0 (or 130 if it had to cut short): {status}"
    );
}

#[tokio::test]
async fn l3_k9_a_route_resumes_once_the_hub_accepts_its_token_again() {
    // G5 (closed at Kenny's ratification): a 401 is not a dead end.
    // Scenario: the hub's door changes under a running runner — the
    // runner keeps its token, backs off, and must pick up by itself
    // once the hub accepts that token again, with no restart.
    let mut hub = Hub::start_with_door("eerste-deurtoken-lang-genoeg").await;
    if !hub.supports_restart() {
        eprintln!("SKIPPED: the token drill needs KYU_BIN (docker keeps no state)");
        return;
    }
    let ha = FakeHa::start();
    let topic = "l3.token";
    let runner_token = "het-token-van-de-runner-zelf";

    let config = route_config(&hub, "doorman", topic, &ha.url("/api/webhook/x"));
    let runner = Runner::start_with_env(&config, &[("KYU_RUNNER_TOKEN", runner_token)]);

    wait_until("the 401 remedy", Duration::from_secs(30), || {
        runner.log().contains("/apps")
    })
    .await;
    assert!(ha.hits().is_empty(), "nothing is delivered while denied");

    // The hub is reconfigured to accept exactly the runner's token.
    hub.stop();
    hub.restart_with_token(runner_token).await;

    // Only publish once the runner is polling again: a subscription
    // created on an ALREADY existing topic starts from now (AR16), so
    // publishing first would hide the very delivery under test.
    wait_until("the runner to poll again", Duration::from_secs(60), || {
        runner.log().contains("topic not born yet")
    })
    .await;
    publish_authed(&hub, Some(runner_token), topic, "text/plain", "na-de-deur").await;

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
