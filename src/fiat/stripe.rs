use crate::currency::{Currency, CurrencyAmount};
use crate::fiat::{FiatPaymentInfo, FiatPaymentService, LineItem};
use crate::webhook::WebhookMessage;
use crate::USER_AGENT;
use anyhow::{Context, Result, anyhow, bail};
use hmac::{Hmac, Mac};
use log::{debug, warn};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, USER_AGENT as USER_AGENT_HEADER};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Form-encoded HTTP client for Stripe API
#[derive(Clone)]
struct FormEncodedApi {
    client: Client,
    base: Url,
    api_key: String,
}

impl FormEncodedApi {
    fn new(base: &str, api_key: String) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT_HEADER, USER_AGENT.parse()?);

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            client,
            base: base.parse()?,
            api_key,
        })
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.base.join(path)?;
        debug!(">> GET {}", url);

        let rsp = self
            .client
            .get(url.clone())
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .send()
            .await?;

        let status = rsp.status();
        let text = rsp.text().await?;
        debug!("<< {} {}", status, text);

        if status.is_success() {
            Ok(serde_json::from_str(&text)?)
        } else {
            bail!("GET {}: {}: {}", path, status, text);
        }
    }

    async fn post<T: serde::de::DeserializeOwned, R: Serialize>(
        &self,
        path: &str,
        body: R,
    ) -> Result<T> {
        let url = self.base.join(path)?;
        let form_body = serde_html_form::to_string(&body)?;
        debug!(">> POST {}: {}", url, form_body);

        let rsp = self
            .client
            .post(url.clone())
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(form_body)
            .send()
            .await?;

        let status = rsp.status();
        let text = rsp.text().await?;
        debug!("<< {} {}", status, text);

        if status.is_success() {
            Ok(serde_json::from_str(&text)?)
        } else {
            bail!("POST {}: {}: {}", path, status, text);
        }
    }

    async fn post_empty<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.base.join(path)?;
        debug!(">> POST {} (empty body)", url);

        let rsp = self
            .client
            .post(url.clone())
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .send()
            .await?;

        let status = rsp.status();
        let text = rsp.text().await?;
        debug!("<< {} {}", status, text);

        if status.is_success() {
            Ok(serde_json::from_str(&text)?)
        } else {
            bail!("POST {}: {}: {}", path, status, text);
        }
    }

    async fn delete<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = self.base.join(path)?;
        debug!(">> DELETE {}", url);

        let rsp = self
            .client
            .delete(url.clone())
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key))
            .send()
            .await?;

        let status = rsp.status();
        let text = rsp.text().await?;
        debug!("<< {} {}", status, text);

        if status.is_success() {
            Ok(serde_json::from_str(&text)?)
        } else {
            bail!("DELETE {}: {}: {}", path, status, text);
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct StripeConfig {
    pub url: Option<String>,
    pub api_key: String,
    pub webhook_secret: Option<String>,
}

#[derive(Clone)]
pub struct StripeApi {
    api: FormEncodedApi,
    webhook_secret: Option<String>,
}

impl StripeApi {
    pub fn new(config: StripeConfig) -> Result<Self> {
        const DEFAULT_URL: &str = "https://api.stripe.com";

        Ok(Self {
            api: FormEncodedApi::new(
                &config.url.unwrap_or(DEFAULT_URL.to_string()),
                config.api_key,
            )?,
            webhook_secret: config.webhook_secret,
        })
    }

    /// Get the webhook secret for verifying incoming webhook events.
    ///
    /// Use this with [`StripeWebhookEvent::verify`] to validate webhook signatures.
    pub fn webhook_secret(&self) -> Option<&str> {
        self.webhook_secret.as_deref()
    }

    /// List all webhook endpoints
    pub async fn list_webhooks(&self) -> Result<StripeWebhookList> {
        self.api.get("/v1/webhook_endpoints").await
    }

    /// Delete a webhook endpoint
    pub async fn delete_webhook(&self, webhook_id: &str) -> Result<StripeWebhook> {
        self.api
            .delete(&format!("/v1/webhook_endpoints/{}", webhook_id))
            .await
    }

    /// Create a webhook endpoint
    pub async fn create_webhook(
        &self,
        url: &str,
        enabled_events: Vec<String>,
    ) -> Result<StripeWebhook> {
        self.api
            .post(
                "/v1/webhook_endpoints",
                CreateWebhookRequest {
                    url: url.to_string(),
                    enabled_events,
                },
            )
            .await
    }

    /// Create a checkout session
    pub async fn create_checkout_session(
        &self,
        request: CreateCheckoutSessionRequest,
    ) -> Result<StripeCheckoutSession> {
        self.api.post("/v1/checkout/sessions", request).await
    }

    /// Retrieve a checkout session
    pub async fn get_checkout_session(&self, session_id: &str) -> Result<StripeCheckoutSession> {
        self.api
            .get(&format!("/v1/checkout/sessions/{}", session_id))
            .await
    }

    /// Update a checkout session (only specific fields can be updated)
    pub async fn update_checkout_session(
        &self,
        session_id: &str,
        request: UpdateCheckoutSessionRequest,
    ) -> Result<StripeCheckoutSession> {
        self.api
            .post(&format!("/v1/checkout/sessions/{}", session_id), request)
            .await
    }

    /// List all checkout sessions
    pub async fn list_checkout_sessions(
        &self,
        limit: Option<u64>,
    ) -> Result<StripeCheckoutSessionList> {
        let path = if let Some(limit) = limit {
            format!("/v1/checkout/sessions?limit={}", limit)
        } else {
            "/v1/checkout/sessions".to_string()
        };
        self.api.get(&path).await
    }

    /// Retrieve line items for a checkout session
    pub async fn get_checkout_session_line_items(
        &self,
        session_id: &str,
    ) -> Result<StripeLineItemList> {
        self.api
            .get(&format!("/v1/checkout/sessions/{}/line_items", session_id))
            .await
    }

    /// Expire a checkout session
    pub async fn expire_checkout_session(&self, session_id: &str) -> Result<StripeCheckoutSession> {
        self.api
            .post_empty(&format!("/v1/checkout/sessions/{}/expire", session_id))
            .await
    }

    /// Create a payment intent (alternative to checkout sessions)
    pub async fn create_payment_intent(
        &self,
        amount: CurrencyAmount,
        description: Option<String>,
    ) -> Result<StripePaymentIntent> {
        let currency = amount.currency().to_string().to_lowercase();

        self.api
            .post(
                "/v1/payment_intents",
                CreatePaymentIntentRequest {
                    amount: match amount.currency() {
                        Currency::BTC => bail!("Bitcoin amount not allowed for fiat payments"),
                        _ => amount.value(),
                    },
                    currency,
                    description,
                    automatic_payment_methods: Some(HashMap::from_iter([(
                        "enabled".to_string(),
                        "true".to_string(),
                    )])),
                    confirm: Some(true),
                },
            )
            .await
    }

    /// Retrieve a payment intent
    pub async fn get_payment_intent(&self, payment_intent_id: &str) -> Result<StripePaymentIntent> {
        self.api
            .get(&format!("/v1/payment_intents/{}", payment_intent_id))
            .await
    }

    /// Cancel a payment intent
    pub async fn cancel_payment_intent(
        &self,
        payment_intent_id: &str,
    ) -> Result<StripePaymentIntent> {
        self.api
            .post_empty(&format!("/v1/payment_intents/{}/cancel", payment_intent_id))
            .await
    }
}

impl FiatPaymentService for StripeApi {
    fn create_order(
        &self,
        description: &str,
        amount: CurrencyAmount,
        line_items: Option<Vec<LineItem>>,
    ) -> Pin<Box<dyn Future<Output = Result<FiatPaymentInfo>> + Send>> {
        let s = self.clone();
        let desc = description.to_string();
        Box::pin(async move {
            // If line items are provided, use Checkout Sessions
            if let Some(items) = line_items {
                let checkout_items: Vec<CheckoutLineItem> = items
                    .into_iter()
                    .map(|item| {
                        // Build product metadata with tax info if present
                        let mut metadata_map = serde_json::Map::new();
                        if let Some(tax_amt) = item.tax_amount {
                            metadata_map.insert("tax_amount".to_string(), serde_json::json!(tax_amt));
                        }
                        if let Some(tax_name) = &item.tax_name {
                            metadata_map.insert("tax_name".to_string(), serde_json::json!(tax_name));
                        }
                        // Merge with existing metadata if any
                        if let Some(serde_json::Value::Object(existing)) = item.metadata {
                            metadata_map.extend(existing);
                        }
                        
                        let metadata = if metadata_map.is_empty() {
                            None
                        } else {
                            Some(serde_json::Value::Object(metadata_map))
                        };

                        CheckoutLineItem {
                            price: None,
                            price_data: Some(PriceData {
                                currency: item.currency.to_lowercase(),
                                unit_amount: item.unit_amount,
                                product_data: ProductData {
                                    name: item.name,
                                    description: item.description,
                                    images: item.images,
                                    metadata,
                                },
                                recurring: None,
                                tax_behavior: Some("exclusive".to_string()), // Tax is added on top
                            }),
                            quantity: item.quantity,
                            tax_rates: None, // Could be enhanced to use Stripe tax rates
                        }
                    })
                    .collect();

                let request = CreateCheckoutSessionRequest {
                    line_items: checkout_items,
                    mode: "payment".to_string(),
                    success_url: None,
                    cancel_url: None,
                    customer_email: None,
                    customer: None,
                    client_reference_id: Some(desc),
                    metadata: None,
                    expires_at: None,
                };

                let rsp = s.create_checkout_session(request).await?;
                Ok(FiatPaymentInfo {
                    raw_data: serde_json::to_string(&rsp)?,
                    external_id: rsp.id,
                })
            } else {
                // Otherwise, use Payment Intents
                let rsp = s.create_payment_intent(amount, Some(desc)).await?;
                Ok(FiatPaymentInfo {
                    raw_data: serde_json::to_string(&rsp)?,
                    external_id: rsp.id,
                })
            }
        })
    }

    fn cancel_order(&self, id: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        let s = self.clone();
        let id = id.to_string();
        Box::pin(async move {
            // Try to cancel as payment intent first
            // If the ID is a checkout session, this will fail and we'll try expiring the session
            if id.starts_with("pi_") {
                s.cancel_payment_intent(&id).await?;
            } else if id.starts_with("cs_") {
                s.expire_checkout_session(&id).await?;
            } else {
                // Try payment intent first, fall back to checkout session
                if s.cancel_payment_intent(&id).await.is_err() {
                    s.expire_checkout_session(&id).await?;
                }
            }
            Ok(())
        })
    }
}

// Request/Response Structures

#[derive(Clone, Serialize)]
struct CreateWebhookRequest {
    pub url: String,
    pub enabled_events: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeWebhook {
    pub id: String,
    pub object: String,
    pub url: String,
    pub enabled_events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    pub status: String,
    pub livemode: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeWebhookList {
    pub object: String,
    pub data: Vec<StripeWebhook>,
    pub has_more: bool,
}

#[derive(Clone, Serialize)]
pub struct CreateCheckoutSessionRequest {
    pub line_items: Vec<CheckoutLineItem>,
    pub mode: String, // "payment", "subscription", or "setup"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct UpdateCheckoutSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Serialize)]
pub struct CheckoutLineItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>, // ID of existing Price object
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_data: Option<PriceData>,
    pub quantity: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_rates: Option<Vec<String>>, // IDs of tax rates to apply
}

#[derive(Clone, Serialize)]
pub struct PriceData {
    pub currency: String,
    pub unit_amount: u64,
    pub product_data: ProductData,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurring: Option<RecurringData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_behavior: Option<String>, // "inclusive", "exclusive", or "unspecified"
}

#[derive(Clone, Serialize)]
pub struct RecurringData {
    pub interval: String, // "day", "week", "month", "year"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_count: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct ProductData {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeCheckoutSession {
    pub id: String,
    pub object: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_subtotal: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_total: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer_email: Option<String>,
    pub payment_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub expires_at: i64,
    pub livemode: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_reference_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_intent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeCheckoutSessionList {
    pub object: String,
    pub data: Vec<StripeCheckoutSession>,
    pub has_more: bool,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeLineItemList {
    pub object: String,
    pub data: Vec<StripeLineItem>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeLineItem {
    pub id: String,
    pub object: String,
    pub amount_subtotal: i64,
    pub amount_total: i64,
    pub currency: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<serde_json::Value>,
    pub quantity: Option<i64>,
}

#[derive(Clone, Serialize)]
pub struct CreatePaymentIntentRequest {
    pub amount: u64,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automatic_payment_methods: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripePaymentIntent {
    pub id: String,
    pub object: String,
    pub amount: u64,
    pub currency: String,
    pub status: StripePaymentIntentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StripePaymentIntentStatus {
    RequiresPaymentMethod,
    RequiresConfirmation,
    RequiresAction,
    Processing,
    RequiresCapture,
    Canceled,
    Succeeded,
}

// Webhook Event Handling

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeWebhookEvent {
    pub id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: StripeEventData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StripeEventData {
    pub object: serde_json::Value,
}

type HmacSha256 = Hmac<sha2::Sha256>;

impl StripeWebhookEvent {
    /// Verify and parse a Stripe webhook event
    pub fn verify(secret: &str, msg: &WebhookMessage) -> Result<Self> {
        let sig_header = msg
            .headers
            .get("stripe-signature")
            .ok_or_else(|| anyhow!("Missing Stripe-Signature header"))?;

        let mut timestamp = None;
        let mut signatures = Vec::new();

        // Parse the Stripe-Signature header
        for part in sig_header.split(',') {
            let mut split = part.split('=');
            let key = split.next().context("Invalid signature format")?;
            let value = split.next().context("Invalid signature format")?;

            match key {
                "t" => timestamp = Some(value),
                "v1" => signatures.push(value),
                _ => {}
            }
        }

        let timestamp = timestamp.ok_or_else(|| anyhow!("Missing timestamp in signature"))?;

        // Construct the signed payload
        let signed_payload = format!("{}.{}", timestamp, String::from_utf8_lossy(&msg.body));

        // Verify the signature
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
        mac.update(signed_payload.as_bytes());
        let result = mac.finalize().into_bytes();
        let expected_sig = hex::encode(result);

        if !signatures.iter().any(|sig| *sig == expected_sig) {
            warn!("Invalid Stripe webhook signature");
            bail!("Invalid signature");
        }

        // Parse the event
        let event: StripeWebhookEvent = serde_json::from_slice(&msg.body)?;
        Ok(event)
    }
}
