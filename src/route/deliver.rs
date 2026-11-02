//! Delivering one claimed message and settling it with the hub: the
//! in-process retry ladder inside the lease budget (AR3/AR5) and the
//! ack/nack calls that end every claim.

use std::time::Instant;

use tokio::sync::watch;
use tracing::{debug, warn};

use super::RouteRunner;
use super::backoff::interrupted;
use crate::config::{DELIVERY_RETRY_BASE, DELIVERY_RETRY_CAP, SETTLE_RETRY_PAUSE};
use crate::hub::Claimed;
use crate::webhook::DeliveryError;

/// How a delivery attempt ended.
pub(super) enum DeliverEnd {
    Delivered,
    /// The budget ran out; `connect_class` says whether the target was
    /// unreachable (which opens the circuit, AR15).
    GiveUp {
        connect_class: bool,
    },
    Shutdown,
}

/// Attempts per settle call: the first try plus one bounded retry.
const SETTLE_ATTEMPTS: u8 = 2;

impl RouteRunner {
    /// AR3: retry in-process on the same claim while the lease budget
    /// allows — the head of the line stays the head of the line (AR2).
    pub(super) async fn deliver_with_budget(
        &self,
        name: &str,
        message: &Claimed,
        shutdown: &mut watch::Receiver<bool>,
    ) -> DeliverEnd {
        let start = Instant::now();
        let mut delay = DELIVERY_RETRY_BASE;
        loop {
            let error = match self
                .webhook
                .deliver(&self.route.webhook_url, self.webhook_timeout, message)
                .await
            {
                Ok(()) => return DeliverEnd::Delivered,
                Err(error) => error,
            };
            let connect_class = matches!(error, DeliveryError::ConnectClass(_));
            if start.elapsed() + delay + self.webhook_timeout > self.lease_budget {
                if !connect_class {
                    // Bounded: at most one line per lease budget per
                    // route (AR6's no-flood property).
                    warn!(
                        route = %name, id = %message.id, %error,
                        "delivery kept failing within the lease budget — nacking; the hub's \
                         retry and dead-letter machinery takes over (K4)"
                    );
                }
                return DeliverEnd::GiveUp { connect_class };
            }
            debug!(
                route = %name, id = %message.id, %error,
                "delivery failed — retrying in-process on the same claim (AR3)"
            );
            if interrupted(delay, shutdown).await {
                return DeliverEnd::Shutdown;
            }
            delay = (delay * 2).min(DELIVERY_RETRY_CAP);
        }
    }

    pub(super) async fn settle_ack(&self, name: &str, message: &Claimed) {
        for attempt in 1..=SETTLE_ATTEMPTS {
            let result = self
                .hub
                .ack(
                    &self.route.topic,
                    &self.route.subscription,
                    &message.id,
                    message.attempt(),
                )
                .await;
            match result {
                Ok(()) => {
                    debug!(route = %name, id = %message.id, bytes = message.body.len(),
                        "delivered and acked");
                    return;
                }
                Err(error) if attempt < SETTLE_ATTEMPTS => {
                    debug!(route = %name, id = %message.id, %error, "ack failed — one retry");
                    tokio::time::sleep(SETTLE_RETRY_PAUSE).await;
                }
                Err(error) => {
                    // A lost ack means a redelivery; the duplicate is
                    // legal by contract (S3, AR13).
                    warn!(
                        route = %name, id = %message.id, %error,
                        "ack failed after a retry — the message will redeliver; consumers \
                         tolerate the duplicate (at-least-once, AR13)"
                    );
                }
            }
        }
    }

    pub(super) async fn settle_nack(&self, name: &str, id: &str, attempt: Option<&str>) {
        self.health.count_nacked(name);
        if let Err(error) = self
            .hub
            .nack(&self.route.topic, &self.route.subscription, id, attempt)
            .await
        {
            debug!(
                route = %name, id = %id, %error,
                "nack failed — the lease will expire and redeliver on its own (kyu K5)"
            );
        }
    }
}
