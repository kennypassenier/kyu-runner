// L2 · the pump (K1-K5) end-to-end: a real kyu hub (scratch), a
// fake HA webhook server, the runner as a real process. Scenario names
// carry the scope's S-ids.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l2_k1_k2_k3_ar16_the_pump_delivers_byte_for_byte_and_acks() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l2.happy";
    let mut runner = Runner::start(
        &hub,
        &route_config(&hub, "happy", topic, &ha.url("/api/webhook/x")),
    );

    // Published AFTER the runner's first poll: the topic is born under
    // the runner's feet, exercising the AR16 replay path — the first
    // message on a brand-new topic must not fall in the gap.
    wait_first_poll(&runner).await;
    let payload = r#"{"title":"Backup done","ok":true}"#;
    publish(&hub, topic, "application/json", payload).await;

    wait_until("the webhook hit", Duration::from_secs(30), || {
        !ha.hits().is_empty()
    })
    .await;
    let hit = ha.hits()[0].clone();
    assert_eq!(hit.method, "POST");
    assert_eq!(hit.path, "/api/webhook/x");
    assert_eq!(hit.body_str(), payload, "byte-for-byte (K2/NG2)");
    assert_eq!(hit.content_type.as_deref(), Some("application/json"));
    let names: Vec<&str> = hit.headers.iter().map(|(name, _)| name.as_str()).collect();
    assert!(names.contains(&"kyu-id"), "metadata passes through");
    assert!(names.contains(&"kyu-attempt"));

    // Acked on the hub (K3): once the runner is gone, the subscription
    // has nothing pending — an unacked message would come back.
    wait_until("the ack to land", Duration::from_secs(10), || {
        runner.log().contains("delivered and acked")
    })
    .await;
    runner.kill_hard();
    assert_eq!(poll_once(&hub, topic, "ha-runner").await, None);
}

#[tokio::test]
async fn l2_s1_a_500_from_ha_is_not_acked_and_the_message_returns() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    ha.set_mode(HaMode::Fail(500));
    let topic = "l2.retry";
    let runner = Runner::start(
        &hub,
        &route_config(&hub, "retry", topic, &ha.url("/api/webhook/x")),
    );

    wait_first_poll(&runner).await;
    publish(&hub, topic, "text/plain", "must-arrive").await;

    // In-process retries on the same claim first (AR3), then the nack
    // hands it back to the hub (K4).
    wait_until(
        "the nack after the lease budget",
        Duration::from_secs(45),
        || runner.log().contains("delivery kept failing"),
    )
    .await;
    ha.set_mode(HaMode::Ok200);
    wait_until("the redelivery to succeed", Duration::from_secs(45), || {
        ha.hits().iter().any(|hit| {
            hit.headers
                .iter()
                .any(|(name, value)| name == "kyu-attempt" && value == "2")
        }) && runner.log().contains("delivered and acked")
    })
    .await;
    let last = ha.hits().last().unwrap().clone();
    assert_eq!(last.body_str(), "must-arrive");
    // Gap audit #9: an up-but-failing target must never open the
    // circuit — the next real message is its only side-effect-free probe.
    assert!(
        !runner.log().contains("circuit open"),
        "a 500 is not a connect-class failure (AR15)"
    );
}

#[tokio::test]
async fn l2_s2_ar15_an_outage_accumulates_unclaimed_and_drains_in_order() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l2.outage";
    let runner = Runner::start(
        &hub,
        &route_config(&hub, "outage", topic, &ha.url("/api/webhook/x")),
    );

    wait_first_poll(&runner).await;
    publish(&hub, topic, "text/plain", "m0").await;
    wait_until("the warm-up delivery", Duration::from_secs(30), || {
        ha.hits().len() == 1
    })
    .await;

    // HA goes down entirely (connection refused).
    let port = ha.shut_down();
    for body in ["m1", "m2", "m3", "m4", "m5"] {
        publish(&hub, topic, "text/plain", body).await;
    }
    wait_until("the circuit to open", Duration::from_secs(60), || {
        runner.log().contains("circuit open")
    })
    .await;

    // HA returns on the same address; the probe closes the circuit and
    // the backlog drains in publish order (S2).
    let ha_back = FakeHa::start_on(port);
    wait_until("the backlog to drain", Duration::from_secs(90), || {
        ha_back.hits().len() == 5
    })
    .await;
    let bodies: Vec<String> = ha_back.hits().iter().map(|hit| hit.body_str()).collect();
    assert_eq!(bodies, ["m1", "m2", "m3", "m4", "m5"], "publish order (S2)");

    // AR15's point: while the circuit was open nothing was claimed, so
    // only the probe message burned an attempt.
    for hit in ha_back.hits().iter().skip(1) {
        let attempt = hit
            .headers
            .iter()
            .find(|(name, _)| name == "kyu-attempt")
            .map(|(_, value)| value.clone())
            .unwrap();
        assert_eq!(attempt, "1", "no attempt burn while the circuit was open");
    }
}

#[tokio::test]
async fn l2_s3_kill_nine_mid_delivery_loses_nothing() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l2.kill";

    // Shrink the lease so the redelivery after the kill is quick; the
    // config carries the same lease for its budget math (AR5). The
    // subscription must exist before a policy can be set (W3 ordering).
    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;
    put_policy(&hub, topic, "ha-runner", r#"{"lease_ms":12000}"#).await;

    let config = route_config(&hub, "kill", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
    );
    ha.set_mode(HaMode::SlowOk(Duration::from_secs(2)));
    let mut runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", "precious").await;

    // The hit is recorded before the slow response completes: kill the
    // runner exactly mid-delivery, after the POST, before the ack (S3).
    wait_until("the delivery to start", Duration::from_secs(30), || {
        ha.hits().iter().any(|hit| hit.body_str() == "precious")
    })
    .await;
    runner.kill_hard();

    ha.set_mode(HaMode::Ok200);
    let runner2 = Runner::start(&hub, &config);
    wait_until("the redelivery", Duration::from_secs(60), || {
        ha.hits()
            .iter()
            .filter(|hit| hit.body_str() == "precious")
            .count()
            >= 2
            && runner2.log().contains("delivered and acked")
    })
    .await;
    // At-least-once: the duplicate is legal, the loss would not be.
}

#[tokio::test]
async fn l2_s4_k4_a_permanently_failing_webhook_dead_letters_visibly() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    ha.set_mode(HaMode::Fail(500));
    let topic = "l2.poison";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;

    // The whole policy lives in the runner config: W3's PUT replaces
    // every field, so a test-side PUT would be silently reverted the
    // moment the runner applies its own block (the critic's warning,
    // demonstrated on ourselves before this line existed).
    let config = route_config(&hub, "poison", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 1000\n\
         policy = { lease_ms = 12000, max_attempts = 2, backoff_ms = 200 }\n\
         webhook_url =",
    );
    let runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", "the-poison-payload").await;

    wait_until("both nacks", Duration::from_secs(60), || {
        runner.log().matches("delivery kept failing").count() >= 2
    })
    .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let dead = dead_letters(&hub, topic, "ha-runner").await;
        if dead.contains("the-poison-payload") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the dead-letter list never showed the message (K6/S4): {dead}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn l2_ar17_a_redirect_is_a_failure_never_followed() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    ha.set_mode(HaMode::Redirect);
    let topic = "l2.redirect";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;
    put_policy(&hub, topic, "ha-runner", r#"{"lease_ms":12000}"#).await;

    let config = route_config(&hub, "redirect", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
    );
    let mut runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", "not-for-elsewhere").await;

    wait_until("retries against the 302", Duration::from_secs(30), || {
        ha.hits().len() >= 2
    })
    .await;
    assert!(
        ha.hits().iter().all(|hit| hit.method == "POST"),
        "a 302 must never turn into a GET (AR17)"
    );
    runner.kill_hard();

    // Not acked: after the lease expires the message is still there.
    tokio::time::sleep(Duration::from_secs(14)).await;
    let returned = poll_once(&hub, topic, "ha-runner").await;
    assert_eq!(
        returned.map(|(_, body)| body),
        Some("not-for-elsewhere".to_string()),
        "a redirected delivery must not be acked"
    );
}

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
async fn l2_k2_ar4_a_binary_payload_survives_byte_for_byte() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l2.binary";
    let runner = Runner::start(
        &hub,
        &route_config(&hub, "binary", topic, &ha.url("/api/webhook/x")),
    );

    wait_first_poll(&runner).await;
    // Deliberately not UTF-8: a lossy string round-trip would corrupt it.
    let payload: Vec<u8> = vec![0x00, 0xff, 0x9f, 0x92, 0x96, 0x00, 0x80, 0x7f];
    publish_bytes(
        &hub,
        topic,
        Some("application/octet-stream"),
        payload.clone(),
    )
    .await;

    wait_until("the binary hit", Duration::from_secs(30), || {
        !ha.hits().is_empty()
    })
    .await;
    let hit = ha.hits()[0].clone();
    assert_eq!(hit.body, payload, "bytes, not a lossy string (K2/NG2)");
    assert_eq!(
        hit.content_type.as_deref(),
        Some("application/octet-stream")
    );
    // Gap audit #4: all four metadata headers, not a spot check (AR4).
    assert_eq!(hit.header("kyu-topic"), Some(topic));
    assert!(hit.header("kyu-id").is_some());
    assert!(hit.header("kyu-attempt").is_some());
    assert!(hit.header("kyu-published-at").is_some());
}

#[tokio::test]
async fn l2_f1_an_oversize_message_is_nacked_and_dead_letters_without_oom() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l2.oversize";

    publish(&hub, topic, "text/plain", "setup").await;
    let (setup_id, _) = poll_once_from(&hub, topic, "ha-runner", true)
        .await
        .expect("setup");
    ack(&hub, topic, "ha-runner", &setup_id).await;

    // Cap at the validation minimum; the policy shortens the cycle.
    let config = route_config(&hub, "oversize", topic, &ha.url("/api/webhook/x"))
        .replace(
            "webhook_url =",
            "webhook_timeout_ms = 1000\n\
             policy = { lease_ms = 12000, max_attempts = 2, backoff_ms = 200 }\n\
             webhook_url =",
        )
        .replace(
            "hub_url =",
            "[defaults]\nmax_body_bytes = 1024\n\nhub_url =",
        );
    // TOML ordering: [defaults] must not swallow hub_url — rebuild.
    let config = format!(
        "hub_url = \"{}\"\n\n[defaults]\nmax_body_bytes = 1024\nwebhook_timeout_ms = 1000\n\n{}",
        hub.base(),
        config
            .lines()
            .skip_while(|line| !line.starts_with("[[routes]]"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let runner = Runner::start(&hub, &config);
    publish(&hub, topic, "text/plain", &"X".repeat(8 * 1024)).await;

    wait_until("the oversize warn", Duration::from_secs(30), || {
        runner.log().contains("larger than max_body_bytes")
    })
    .await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if !dead_letters(&hub, topic, "ha-runner")
            .await
            .contains("\"dead_letters\":[]")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the oversize message never dead-lettered"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(ha.hits().is_empty(), "an oversize body is never forwarded");
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
