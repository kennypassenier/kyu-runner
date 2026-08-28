//! hub-bridge: a stateless pump from the mailbox hub to Home Assistant
//! webhooks. It long-polls configured topic subscriptions, forwards each
//! payload byte-for-byte to an HA webhook, and acks only on HA's 2xx —
//! so the hub's retry → dead-letter machinery works for the HA delivery.

fn main() {
    println!("hub-bridge {}", env!("CARGO_PKG_VERSION"));
}
