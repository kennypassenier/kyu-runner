// L4 · the mailbox.events route (K6), declarative policy (W3) and the
// health endpoint (W4), E2E.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l4_k6_a_dead_letter_event_reaches_the_warning_webhook() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let bridge = Bridge::start(&route_config(
        &hub,
        "mailbox-events",
        "mailbox.events",
        &ha.url("/api/webhook/hub_mailbox_events"),
    ));

    // mailbox.events already exists on a fresh hub, so there is no
    // unborn line to wait for — wait until the bridge's first poll has
    // created the subscription instead.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !subscription_exists(&hub, "mailbox.events", "ha-bridge").await {
        assert!(
            std::time::Instant::now() < deadline,
            "the bridge never created its mailbox.events subscription"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = &bridge;

    // Manufacture a dead letter through the sweeper: the hub emits
    // `message.dead_lettered` only when the sweeper settles an overdue
    // delivery whose attempts ran out (a poison-pill nack marks dead
    // without an event — verified in the engine).
    publish(&hub, "l4.victim", "text/plain", "opener").await;
    let (opener_id, _) = poll_once_from(&hub, "l4.victim", "victim", true)
        .await
        .expect("claim the opener");
    ack(&hub, "l4.victim", "victim", &opener_id).await;
    put_policy(
        &hub,
        "l4.victim",
        "victim",
        r#"{"lease_ms":5000,"max_attempts":1}"#,
    )
    .await;
    publish(&hub, "l4.victim", "text/plain", "doomed").await;
    let _claimed = poll_once(&hub, "l4.victim", "victim")
        .await
        .expect("claim the doomed message");
    // Never settled: the 5 s lease expires, the single attempt is
    // spent, the sweeper dead-letters it and emits the event.

    wait_until("the warning webhook hit", Duration::from_secs(30), || {
        ha.hits()
            .iter()
            .any(|hit| hit.body.contains("dead_lettered"))
    })
    .await;
    let hit = ha
        .hits()
        .iter()
        .find(|hit| hit.body.contains("dead_lettered"))
        .unwrap()
        .clone();
    assert!(
        hit.body.contains("l4.victim"),
        "the event names the topic the dead letter lies on: {}",
        hit.body
    );
}

#[tokio::test]
async fn l4_w3_the_configured_policy_lands_on_the_hub_and_is_logged() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l4.policy";
    let config = route_config(&hub, "policied", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "policy = { lease_ms = 45000, max_attempts = 25 }\nwebhook_url =",
    );
    let bridge = Bridge::start(&config);

    wait_first_poll(&bridge).await;
    // The subscription exists only after a successful poll on a born
    // topic (W3 ordering); the publish births the topic.
    publish(&hub, topic, "text/plain", "first").await;

    wait_until("the policy to land", Duration::from_secs(30), || {
        bridge.log().contains("policy in force")
    })
    .await;
    let policy = get_policy(&hub, topic, "ha-bridge").await;
    assert!(
        policy.contains("45000") && policy.contains("25"),
        "the hub reports the configured values in force (W3): {policy}"
    );
}

#[tokio::test]
async fn l4_w4_healthz_reports_route_states_when_opted_in() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l4.health";
    let port = {
        // A free port for the health listener, distinct from hub/ha.
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let config = route_config(&hub, "healthy", topic, &ha.url("/api/webhook/x")).replace(
        "hub_url =",
        &format!("healthz_listen = \"127.0.0.1:{port}\"\nhub_url ="),
    );
    let bridge = Bridge::start(&config);
    wait_first_poll(&bridge).await;

    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await
        .expect("healthz reachable");
    assert!(response.status().is_success());
    let body = response.text().await.expect("healthz body");
    assert!(
        body.contains("\"name\":\"healthy\"") && body.contains("\"state\""),
        "route names and loop states, nothing else (AR10): {body}"
    );
}
