// L2 · the pump (K1-K5) end-to-end: a real kyu hub (scratch), a
// fake HA webhook server, the runner as a real process. Scenario names
// carry the scope's S-ids.
// This part: failures that end in a dead letter or a refusal.

mod support;

use std::time::Duration;

use support::*;

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
