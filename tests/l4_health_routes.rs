// L4 · the kyu.events route (K6), declarative policy (W3) and the
// health endpoint (W4), E2E.
// This part: the health endpoint and route independence.

mod support;

use std::time::Duration;

use support::*;

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
    let runner = Runner::start(&hub, &config);
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
    let runner = Runner::start(&hub, &config);
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
    let runner = Runner::start(&hub, &without);
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
    let runner = Runner::start(&hub, &with);
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
    let runner = Runner::start(&hub, &config);
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
