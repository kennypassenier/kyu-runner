// L4 · the kyu.events route (K6), declarative policy (W3) and the
// health endpoint (W4), E2E.

mod support;

use std::time::Duration;

use support::*;

#[tokio::test]
async fn l4_k6_a_dead_letter_event_reaches_the_warning_webhook() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let runner = Runner::start(&route_config(
        &hub,
        "kyu-events",
        "kyu.events",
        &ha.url("/api/webhook/hub_kyu_events"),
    ));

    // kyu.events already exists on a fresh hub, so there is no
    // unborn line to wait for — wait until the runner's first poll has
    // created the subscription instead.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !subscription_exists(&hub, "kyu.events", "ha-runner").await {
        assert!(
            std::time::Instant::now() < deadline,
            "the runner never created its kyu.events subscription"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = &runner;

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
            .any(|hit| hit.body_str().contains("dead_lettered"))
    })
    .await;
    let hit = ha
        .hits()
        .iter()
        .find(|hit| hit.body_str().contains("dead_lettered"))
        .unwrap()
        .clone();
    assert!(
        hit.body_str().contains("l4.victim"),
        "the event names the topic the dead letter lies on: {}",
        hit.body_str()
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
    let runner = Runner::start(&config);

    wait_first_poll(&runner).await;
    // The subscription exists only after a successful poll on a born
    // topic (W3 ordering); the publish births the topic.
    publish(&hub, topic, "text/plain", "first").await;

    wait_until("the policy to land", Duration::from_secs(30), || {
        runner.log().contains("policy in force")
    })
    .await;
    let policy = get_policy(&hub, topic, "ha-runner").await;
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
    let runner = Runner::start(&config);
    wait_first_poll(&runner).await;

    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await
        .expect("healthz reachable");
    assert!(response.status().is_success());
    let body = response.text().await.expect("healthz body");
    assert!(
        body.contains("\"healthy\"") && body.contains("\"detail\""),
        "route names and loop states as the kit's subsystems (AR10): {body}"
    );
}

#[tokio::test]
async fn l4_ar16_a_pre_existing_topic_starts_from_now_not_from_history() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l4.history";

    // Three messages exist BEFORE the route's first poll; backfilling
    // them into HA would be months-of-TTS-replayed in production.
    for body in ["h1", "h2", "h3"] {
        publish(&hub, topic, "text/plain", body).await;
    }
    let runner = Runner::start(&route_config(
        &hub,
        "historian",
        topic,
        &ha.url("/api/webhook/x"),
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !subscription_exists(&hub, topic, "ha-runner").await {
        assert!(std::time::Instant::now() < deadline, "no subscription");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        ha.hits().is_empty(),
        "a pre-existing topic starts from now (AR16): {:?}",
        ha.hits()
    );

    publish(&hub, topic, "text/plain", "h4").await;
    wait_until("only the new message", Duration::from_secs(30), || {
        ha.hits().len() == 1
    })
    .await;
    assert_eq!(ha.hits()[0].body_str(), "h4");
    let _ = &runner;
}

#[tokio::test]
async fn l4_k1_two_routes_run_independently_through_one_outage() {
    let hub = Hub::start().await;
    let ha_alpha = FakeHa::start();
    let ha_beta = FakeHa::start();
    let config = format!(
        "hub_url = \"{}\"\n\n\
         [[routes]]\nname = \"alpha\"\ntopic = \"l4.alpha\"\nsubscription = \"ha-runner\"\n\
         webhook_url = \"{}\"\n\n\
         [[routes]]\nname = \"beta\"\ntopic = \"l4.beta\"\nsubscription = \"ha-runner\"\n\
         webhook_url = \"{}\"\n",
        hub.base(),
        ha_alpha.url("/api/webhook/alpha"),
        ha_beta.url("/api/webhook/beta"),
    );
    let runner = Runner::start(&config);
    wait_until("both first polls", Duration::from_secs(15), || {
        let log = runner.log();
        log.matches("topic not born yet").count() >= 2
    })
    .await;

    publish(&hub, "l4.alpha", "text/plain", "a1").await;
    publish(&hub, "l4.beta", "text/plain", "b1").await;
    wait_until("both warm-ups", Duration::from_secs(30), || {
        !ha_alpha.hits().is_empty() && !ha_beta.hits().is_empty()
    })
    .await;

    // Alpha's target dies; beta must keep flowing while alpha's
    // circuit is open (K1: no cross-route head-of-line blocking).
    let _dead_port = ha_alpha.shut_down();
    publish(&hub, "l4.alpha", "text/plain", "a2").await;
    wait_until("alpha's circuit", Duration::from_secs(60), || {
        runner.log().contains("circuit open")
    })
    .await;
    for body in ["b2", "b3", "b4"] {
        publish(&hub, "l4.beta", "text/plain", body).await;
    }
    wait_until("beta unaffected", Duration::from_secs(30), || {
        ha_beta.hits().len() == 4
    })
    .await;
    let bodies: Vec<String> = ha_beta.hits().iter().map(|hit| hit.body_str()).collect();
    assert_eq!(bodies, ["b1", "b2", "b3", "b4"]);
}

#[tokio::test]
async fn l4_w3_a_hub_refused_policy_warns_once_and_the_route_keeps_delivering() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l4.badpolicy";
    // max_attempts = 0 passes the runner's own validation (it only
    // bounds lease_ms) but the hub refuses it with a remedy — the
    // route must warn once and keep running under the hub's defaults.
    let config = route_config(&hub, "badpolicy", topic, &ha.url("/api/webhook/x")).replace(
        "webhook_url =",
        "policy = { max_attempts = 0 }\nwebhook_url =",
    );
    let runner = Runner::start(&config);
    wait_first_poll(&runner).await;
    publish(&hub, topic, "text/plain", "still-flows").await;

    wait_until(
        "the policy warn + delivery",
        Duration::from_secs(30),
        || {
            let log = runner.log();
            log.contains("policy PUT failed")
                && ha.hits().iter().any(|hit| hit.body_str() == "still-flows")
        },
    )
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        runner.log().matches("policy PUT failed").count(),
        1,
        "warn once, then quiet (AR6)"
    );
}

#[tokio::test]
async fn l4_w4_the_config_key_is_what_opens_the_socket() {
    // G7 (closed at Kenny's ratification 2026-08-30): the fail-closed
    // half. The same config, once without and once with the key, on the
    // same port — the contrast is the evidence that nothing listens
    // unless you ask for it.
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l4.optin";
    let port = free_port();

    let without = route_config(&hub, "silent", topic, &ha.url("/api/webhook/x"));
    let runner = Runner::start(&without);
    wait_first_poll(&runner).await;
    assert!(
        std::net::TcpStream::connect(("127.0.0.1", port)).is_err(),
        "no healthz_listen key must mean no socket at all"
    );
    drop(runner);

    let with = without.replace(
        "hub_url =",
        &format!("healthz_listen = \"127.0.0.1:{port}\"\nhub_url ="),
    );
    let runner = Runner::start(&with);
    wait_first_poll(&runner).await;
    let response = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await
        .expect("with the key the socket answers");
    assert!(response.status().is_success());
}

#[tokio::test]
async fn l4_w4_healthz_reports_the_failure_state_not_a_frozen_ok() {
    // G7, second half: monitoring that always says the same thing is
    // worse than no monitoring — it is the exact failure this project
    // (P8) exists to prevent. So the state must actually move when the
    // hub goes away.
    let mut hub = Hub::start().await;
    if !hub.supports_restart() {
        eprintln!("SKIPPED: stopping the hub needs KYU_BIN (docker keeps no state)");
        return;
    }
    let ha = FakeHa::start();
    let topic = "l4.states";
    let port = free_port();
    let config = route_config(&hub, "watched", topic, &ha.url("/api/webhook/x")).replace(
        "hub_url =",
        &format!("healthz_listen = \"127.0.0.1:{port}\"\nhub_url ="),
    );
    let runner = Runner::start(&config);
    wait_first_poll(&runner).await;

    let state = || async {
        reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await
            .expect("healthz answers")
            .text()
            .await
            .expect("healthz body")
    };
    let healthy = state().await;
    assert!(
        !healthy.contains("hub-down"),
        "a reachable hub is not reported as down: {healthy}"
    );

    hub.stop();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        if state().await.contains("hub-down") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "healthz never left its healthy state while the hub was down: {}",
            state().await
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let _ = &runner;
}
