//! Hub-side test verbs (the dashboard-documented API): publish, poll,
//! ack, policy and dead-letter helpers against a scratch [`Hub`].

use super::hub::Hub;

pub async fn publish(hub: &Hub, topic: &str, content_type: &str, body: &str) {
    publish_authed(hub, Some(hub.client_token()), topic, content_type, body).await;
}

/// Raw publish: arbitrary bytes, optionally without a content-type.
pub async fn publish_bytes(hub: &Hub, topic: &str, content_type: Option<&str>, body: Vec<u8>) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/t/{topic}", hub.base()))
        .bearer_auth(hub.client_token())
        .body(body);
    if let Some(content_type) = content_type {
        request = request.header("content-type", content_type);
    }
    let response = request.send().await.expect("publish bytes");
    assert_eq!(response.status().as_u16(), 201, "publish should be 201");
}

pub async fn publish_authed(
    hub: &Hub,
    token: Option<&str>,
    topic: &str,
    content_type: &str,
    body: &str,
) {
    let mut request = reqwest::Client::new()
        .post(format!("{}/t/{topic}", hub.base()))
        .header("content-type", content_type)
        .body(body.to_string());
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.expect("publish");
    assert_eq!(response.status().as_u16(), 201, "publish should be 201");
}

/// Poll once with wait=0 as the given subscription; returns the body if
/// a message was pending. Used to prove acks (a drained queue answers
/// 204) and to pre-create subscriptions.
pub async fn poll_once(hub: &Hub, topic: &str, subscription: &str) -> Option<(String, String)> {
    poll_once_from(hub, topic, subscription, false).await
}

/// `from_beginning` replays retained history — needed when the test
/// publishes before the subscription's first poll (a subscription only
/// sees what follows its creation).
pub async fn poll_once_from(
    hub: &Hub,
    topic: &str,
    subscription: &str,
    from_beginning: bool,
) -> Option<(String, String)> {
    let from = if from_beginning {
        "&from=beginning"
    } else {
        ""
    };
    let response = reqwest::Client::new()
        .get(format!(
            "{}/t/{topic}/next?as={subscription}&wait=0{from}",
            hub.base()
        ))
        .bearer_auth(hub.client_token())
        .send()
        .await
        .expect("poll");
    match response.status().as_u16() {
        204 | 404 => None,
        200 => {
            let id = response
                .headers()
                .get("kyu-id")
                .and_then(|value| value.to_str().ok())
                .expect("kyu-id")
                .to_string();
            let body = response.text().await.expect("body");
            Some((id, body))
        }
        other => panic!("poll answered {other}"),
    }
}

pub async fn ack(hub: &Hub, topic: &str, subscription: &str, id: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/t/{topic}/ack/{id}?as={subscription}",
            hub.base()
        ))
        .bearer_auth(hub.client_token())
        .send()
        .await
        .expect("ack");
    assert!(response.status().is_success(), "ack failed");
}

pub async fn put_policy(hub: &Hub, topic: &str, subscription: &str, policy: &str) {
    let response = reqwest::Client::new()
        .put(format!(
            "{}/api/t/{topic}/subs/{subscription}/policy",
            hub.base()
        ))
        .bearer_auth(hub.admin_token())
        .header("content-type", "application/json")
        .body(policy.to_string())
        .send()
        .await
        .expect("put policy");
    assert!(
        response.status().is_success(),
        "policy PUT failed: {}",
        response.text().await.unwrap_or_default()
    );
}

/// The poison pill (kyu W5): straight to the dead-letter list —
/// the quickest way to make the hub emit a `message.dead_lettered`
/// event for the K6 test.
pub async fn nack_dead(hub: &Hub, topic: &str, subscription: &str, id: &str) {
    let response = reqwest::Client::new()
        .post(format!(
            "{}/t/{topic}/nack/{id}?as={subscription}&dead=true",
            hub.base()
        ))
        .bearer_auth(hub.client_token())
        .send()
        .await
        .expect("nack dead");
    assert!(response.status().is_success(), "poison-pill nack failed");
}

pub async fn get_policy(hub: &Hub, topic: &str, subscription: &str) -> String {
    reqwest::Client::new()
        .get(format!(
            "{}/api/t/{topic}/subs/{subscription}/policy",
            hub.base()
        ))
        .bearer_auth(hub.admin_token())
        .send()
        .await
        .expect("get policy")
        .text()
        .await
        .expect("policy body")
}

/// True once the subscription exists on the hub (its policy endpoint
/// answers 200) — the deterministic "has the runner polled yet?" probe
/// for topics that already exist at hub start (e.g. kyu.events).
pub async fn subscription_exists(hub: &Hub, topic: &str, subscription: &str) -> bool {
    reqwest::Client::new()
        .get(format!(
            "{}/api/t/{topic}/subs/{subscription}/policy",
            hub.base()
        ))
        .bearer_auth(hub.admin_token())
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

pub async fn dead_letters(hub: &Hub, topic: &str, subscription: &str) -> String {
    reqwest::Client::new()
        .get(format!(
            "{}/api/t/{topic}/subs/{subscription}/dead",
            hub.base()
        ))
        .bearer_auth(hub.admin_token())
        .send()
        .await
        .expect("dead letters")
        .text()
        .await
        .expect("dead letter body")
}
