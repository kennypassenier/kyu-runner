// L6 · W6: Prometheus counters on the shared observation socket, E2E.

mod support;

use std::time::Duration;

use support::*;

async fn metrics(port: u16) -> String {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/metrics"))
        .send()
        .await
        .expect("metrics reachable")
        .text()
        .await
        .expect("metrics body")
}

#[tokio::test]
async fn l6_w6_counters_move_on_delivery_and_on_nack() {
    let hub = Hub::start().await;
    let ha = FakeHa::start();
    let topic = "l6.counted";
    let port = {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    // Short lease so the nack half of the test stays quick (AR5 math:
    // budget = 8.4 s with a 3 s webhook timeout).
    let config = route_config(&hub, "counted", topic, &ha.url("/api/webhook/x"))
        .replace(
            "webhook_url =",
            "webhook_timeout_ms = 3000\npolicy = { lease_ms = 12000 }\nwebhook_url =",
        )
        .replace(
            "hub_url =",
            &format!("healthz_listen = \"127.0.0.1:{port}\"\nhub_url ="),
        );
    let bridge = Bridge::start(&config);
    wait_first_poll(&bridge).await;

    // Baseline: both counters exist at zero, and /healthz still
    // answers on the same socket.
    let baseline = metrics(port).await;
    assert!(
        baseline.contains("hub_bridge_delivered_total{route=\"counted\"} 0"),
        "{baseline}"
    );
    assert!(
        baseline.contains("hub_bridge_nacked_total{route=\"counted\"} 0"),
        "{baseline}"
    );
    let health = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/healthz"))
        .send()
        .await
        .expect("healthz reachable")
        .text()
        .await
        .expect("healthz body");
    assert!(health.contains("\"name\":\"counted\""), "{health}");

    // One delivery → delivered_total 1.
    publish(&hub, topic, "text/plain", "count-me").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if metrics(port)
            .await
            .contains("hub_bridge_delivered_total{route=\"counted\"} 1")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "delivered_total never reached 1"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // One failing delivery → nacked_total moves.
    ha.set_mode(HaMode::Fail(500));
    publish(&hub, topic, "text/plain", "refuse-me").await;
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let text = metrics(port).await;
        let nacked = text
            .lines()
            .find(|line| line.starts_with("hub_bridge_nacked_total{route=\"counted\"}"))
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if nacked >= 1 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "nacked_total never moved: {text}"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}
