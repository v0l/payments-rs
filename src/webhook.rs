//! Webhook handling utilities.
//!
//! This module provides a global webhook bridge for routing incoming webhook
//! messages from payment providers (Stripe, Revolut, Bitvora) to their respective
//! handlers.
//!
//! # Example
//!
//! ```rust,ignore
//! use payments_rs::webhook::{WEBHOOK_BRIDGE, WebhookMessage};
//!
//! // In your webhook endpoint handler:
//! let msg = WebhookMessage {
//!     endpoint: "/webhooks/stripe".to_string(),
//!     body: request_body,
//!     headers: request_headers,
//! };
//! WEBHOOK_BRIDGE.send(msg);
//!
//! // In your payment handler:
//! let mut rx = WEBHOOK_BRIDGE.listen();
//! while let Ok(msg) = rx.recv().await {
//!     // Process webhook message
//! }
//! ```

use log::warn;
use std::collections::HashMap;
use std::sync::LazyLock;
#[cfg(feature = "rocket")]
use rocket::Data;
#[cfg(feature = "rocket")]
use rocket::data::FromData;
#[cfg(feature = "rocket")]
use rocket::data::ToByteUnit;
#[cfg(feature = "rocket")]
use rocket::http::Status;
use tokio::sync::broadcast;

/// Global webhook message bridge.
///
/// Use this to route incoming webhook HTTP requests to payment provider handlers.
pub static WEBHOOK_BRIDGE: LazyLock<WebhookBridge> = LazyLock::new(WebhookBridge::new);

/// A webhook message received from a payment provider.
#[derive(Debug, Clone)]
pub struct WebhookMessage {
    /// The endpoint path that received the webhook
    pub endpoint: String,
    /// Raw request body
    pub body: Vec<u8>,
    /// HTTP headers (used for signature verification)
    pub headers: HashMap<String, String>,
}

#[cfg(feature = "rocket")]
#[rocket::async_trait]
impl<'r> FromData<'r> for WebhookMessage {
    type Error = ();

    async fn from_data(
        req: &'r rocket::Request<'_>,
        data: Data<'r>,
    ) -> rocket::data::Outcome<'r, Self, Self::Error> {
        let header = req
            .headers()
            .iter()
            .map(|v| (v.name.to_string(), v.value.to_string()))
            .collect();
        let body = if let Ok(d) = data.open(4.megabytes()).into_bytes().await {
            d
        } else {
            return rocket::data::Outcome::Error((Status::BadRequest, ()));
        };
        let msg = WebhookMessage {
            endpoint: req.uri().path().to_string(),
            headers: header,
            body: body.value.to_vec(),
        };
        rocket::data::Outcome::Success(msg)
    }
}
/// Broadcast bridge for routing webhook messages to handlers.
#[derive(Debug)]
pub struct WebhookBridge {
    tx: broadcast::Sender<WebhookMessage>,
}

impl Default for WebhookBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookBridge {
    /// Create a new webhook bridge with a buffer of 100 messages.
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(100);
        Self { tx }
    }

    /// Send a webhook message to all listeners.
    ///
    /// Messages are dropped if no listeners are subscribed.
    pub fn send(&self, message: WebhookMessage) {
        if let Err(e) = self.tx.send(message) {
            warn!("Failed to send webhook message: {}", e);
        }
    }

    /// Subscribe to receive webhook messages.
    ///
    /// Returns a receiver that will receive all future messages.
    pub fn listen(&self) -> broadcast::Receiver<WebhookMessage> {
        self.tx.subscribe()
    }
}
