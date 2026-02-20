use crate::currency::{Currency, CurrencyAmount};
use crate::fiat::{FiatPaymentInfo, FiatPaymentService, LineItem};
use crate::json_api::{JsonApi, TokenGen};
use crate::webhook::WebhookMessage;
use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use log::warn;
use reqwest::header::AUTHORIZATION;
use reqwest::{Method, RequestBuilder, Url};
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct RevolutConfig {
    pub url: Option<String>,
    pub api_version: String,
    pub token: String,
    pub public_key: String,
}

#[derive(Clone)]
pub struct RevolutApi {
    api: JsonApi,
}

#[derive(Clone)]
struct RevolutTokenGen {
    pub token: String,
    pub api_version: String,
}

impl TokenGen for RevolutTokenGen {
    fn generate_token(
        &self,
        _method: Method,
        _url: &Url,
        _body: Option<&str>,
        req: RequestBuilder,
    ) -> Result<RequestBuilder> {
        Ok(req
            .header(AUTHORIZATION, format!("Bearer {}", &self.token))
            .header("Revolut-Api-Version", &self.api_version))
    }
}

impl RevolutApi {
    pub fn new(config: RevolutConfig) -> Result<Self> {
        let token_gen = RevolutTokenGen {
            token: config.token,
            api_version: config.api_version,
        };
        const DEFAULT_URL: &str = "https://merchant.revolut.com";

        Ok(Self {
            api: JsonApi::token_gen(
                &config.url.unwrap_or(DEFAULT_URL.to_string()),
                false,
                token_gen,
            )?,
        })
    }

    pub async fn list_webhooks(&self) -> Result<Vec<RevolutWebhook>> {
        self.api.get("/api/1.0/webhooks").await
    }

    pub async fn delete_webhook(&self, webhook_id: &str) -> Result<()> {
        self.api
            .req_status::<()>(
                Method::DELETE,
                &format!("/api/1.0/webhooks/{}", webhook_id),
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn create_webhook(
        &self,
        url: &str,
        events: Vec<RevolutWebhookEvent>,
    ) -> Result<RevolutWebhook> {
        self.api
            .post(
                "/api/1.0/webhooks",
                CreateWebhookRequest {
                    url: url.to_string(),
                    events,
                },
            )
            .await
    }

    pub async fn create_order(
        &self,
        amount: CurrencyAmount,
        description: Option<String>,
        line_items: Option<Vec<LineItem>>,
    ) -> Result<RevolutOrder> {
        // Convert generic LineItems to Revolut's format
        let revolut_line_items = line_items.map(|items| {
            items
                .into_iter()
                .map(|item| {
                    let total = item.total_amount();
                    
                    // Build taxes array if tax info is provided
                    let taxes = if let (Some(tax_amt), Some(tax_name)) = (item.tax_amount, item.tax_name) {
                        Some(vec![RevolutTax {
                            name: tax_name,
                            amount: tax_amt,
                        }])
                    } else {
                        None
                    };
                    
                    RevolutLineItem {
                        name: item.name,
                        description: item.description,
                        item_type: None, // Could be enhanced to detect from metadata
                        quantity: RevolutQuantity {
                            value: item.quantity,
                            unit: None, // Could be enhanced to extract from metadata
                        },
                        unit_price_amount: item.unit_amount,
                        total_amount: total,
                        external_id: None,
                        discounts: None,
                        taxes,
                        image_urls: item.images,
                        url: None,
                    }
                })
                .collect()
        });

        self.api
            .post(
                "/api/orders",
                CreateOrderRequest {
                    currency: amount.currency().to_string(),
                    amount: match amount.currency() {
                        Currency::BTC => bail!("Bitcoin amount not allowed for fiat payments"),
                        _ => amount.value(),
                    },
                    description,
                    line_items: revolut_line_items,
                },
            )
            .await
    }

    pub async fn get_order(&self, order_id: &str) -> Result<RevolutOrder> {
        self.api.get(&format!("/api/orders/{}", order_id)).await
    }

    pub async fn cancel_order(&self, order_id: &str) -> Result<RevolutOrder> {
        self.api
            .req::<_, ()>(
                Method::POST,
                &format!("/api/orders/{}/cancel", order_id),
                None,
            )
            .await
    }
}

impl FiatPaymentService for RevolutApi {
    fn create_order(
        &self,
        description: &str,
        amount: CurrencyAmount,
        line_items: Option<Vec<LineItem>>,
    ) -> Pin<Box<dyn Future<Output = Result<FiatPaymentInfo>> + Send>> {
        let s = self.clone();
        let desc = description.to_string();
        Box::pin(async move {
            let rsp = s.create_order(amount, Some(desc), line_items).await?;
            Ok(FiatPaymentInfo {
                raw_data: serde_json::to_string(&rsp)?,
                external_id: rsp.id,
            })
        })
    }

    fn cancel_order(&self, id: &str) -> Pin<Box<dyn Future<Output = Result<()>> + Send>> {
        let s = self.clone();
        let id = id.to_string();
        Box::pin(async move {
            s.cancel_order(&id).await?;
            Ok(())
        })
    }
}

#[derive(Clone, Serialize)]
pub struct CreateOrderRequest {
    pub amount: u64,
    pub currency: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_items: Option<Vec<RevolutLineItem>>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RevolutLineItem {
    pub name: String,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    
    #[serde(rename = "type")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_type: Option<RevolutLineItemType>,
    
    pub quantity: RevolutQuantity,
    
    pub unit_price_amount: u64,
    
    pub total_amount: u64,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discounts: Option<Vec<RevolutDiscount>>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taxes: Option<Vec<RevolutTax>>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_urls: Option<Vec<String>>,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RevolutLineItemType {
    Physical,
    Digital,
    Service,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RevolutQuantity {
    pub value: u64,
    
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RevolutDiscount {
    pub name: String,
    pub amount: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RevolutTax {
    pub name: String,
    pub amount: u64,
}

impl RevolutLineItem {
    /// Create a simple line item with just name, quantity, and pricing
    pub fn simple(name: String, quantity: u64, unit_price_amount: u64) -> Self {
        Self {
            name,
            description: None,
            item_type: None,
            quantity: RevolutQuantity {
                value: quantity,
                unit: None,
            },
            unit_price_amount,
            total_amount: quantity * unit_price_amount,
            external_id: None,
            discounts: None,
            taxes: None,
            image_urls: None,
            url: None,
        }
    }

    /// Builder-style method to set the item type
    pub fn with_type(mut self, item_type: RevolutLineItemType) -> Self {
        self.item_type = Some(item_type);
        self
    }

    /// Builder-style method to set the description
    pub fn with_description(mut self, description: String) -> Self {
        self.description = Some(description);
        self
    }

    /// Builder-style method to set the quantity unit
    pub fn with_unit(mut self, unit: String) -> Self {
        self.quantity.unit = Some(unit);
        self
    }

    /// Builder-style method to add discounts
    pub fn with_discounts(mut self, discounts: Vec<RevolutDiscount>) -> Self {
        // Recalculate total with discounts
        let discount_total: u64 = discounts.iter().map(|d| d.amount).sum();
        self.total_amount = (self.quantity.value * self.unit_price_amount).saturating_sub(discount_total);
        self.discounts = Some(discounts);
        self
    }

    /// Builder-style method to add taxes
    pub fn with_taxes(mut self, taxes: Vec<RevolutTax>) -> Self {
        let tax_total: u64 = taxes.iter().map(|t| t.amount).sum();
        self.total_amount += tax_total;
        self.taxes = Some(taxes);
        self
    }

    /// Builder-style method to set image URLs
    pub fn with_images(mut self, image_urls: Vec<String>) -> Self {
        self.image_urls = Some(image_urls);
        self
    }

    /// Builder-style method to set the product URL
    pub fn with_url(mut self, url: String) -> Self {
        self.url = Some(url);
        self
    }

    /// Builder-style method to set the external ID
    pub fn with_external_id(mut self, external_id: String) -> Self {
        self.external_id = Some(external_id);
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutOrder {
    pub id: String,
    pub token: String,
    pub state: RevolutOrderState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub description: Option<String>,
    pub amount: u64,
    pub currency: String,
    pub outstanding_amount: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkout_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payments: Option<Vec<RevolutOrderPayment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_items: Option<Vec<RevolutLineItem>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutOrderPayment {
    pub id: String,
    pub state: RevolutPaymentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decline_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank_message: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub amount: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_method: Option<RevolutPaymentMethod>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub billing_address: Option<RevolutBillingAddress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<RevolutRiskLevel>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutPaymentMethod {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: RevolutPaymentMethodType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_country_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_bin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_last_four: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_expiry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cardholder_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevolutPaymentMethodType {
    ApplePay,
    Card,
    GooglePay,
    RevolutPayCard,
    RevolutPayAccount,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevolutRiskLevel {
    High,
    Low,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutBillingAddress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street_line_1: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street_line_2: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,

    pub country_code: String,
    pub postcode: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RevolutOrderState {
    Pending,
    Processing,
    Authorised,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevolutPaymentState {
    Pending,
    AuthenticationChallenge,
    AuthenticationVerified,
    AuthorisationStarted,
    AuthorisationPassed,
    Authorised,
    CaptureStarted,
    Captured,
    RefundValidated,
    RefundStarted,
    CancellationStarted,
    Declining,
    Completing,
    Cancelling,
    Failing,
    Completed,
    Declined,
    SoftDeclined,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutWebhook {
    pub id: String,
    pub url: String,
    pub events: Vec<RevolutWebhookEvent>,
    pub signing_secret: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutWebhookBody {
    pub event: RevolutWebhookEvent,
    pub order_id: String,
    pub merchant_order_ext_ref: Option<String>,
}

type HmacSha256 = Hmac<sha2::Sha256>;
impl RevolutWebhookBody {
    pub fn verify(secret: &str, msg: &WebhookMessage) -> Result<Self> {
        let sig = msg
            .headers
            .get("revolut-signature")
            .ok_or_else(|| anyhow!("Missing Revolut-Signature header"))?;
        let timestamp = msg
            .headers
            .get("revolut-request-timestamp")
            .ok_or_else(|| anyhow!("Missing Revolut-Request-Timestamp header"))?;

        // check if any signatures match
        for sig in sig.split(",") {
            let mut sig_split = sig.split("=");
            let (version, code) = (
                sig_split.next().context("Invalid signature format")?,
                sig_split.next().context("Invalid signature format")?,
            );
            let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
            mac.update(version.as_bytes());
            mac.update(b".");
            mac.update(timestamp.as_bytes());
            mac.update(b".");
            mac.update(msg.body.as_slice());
            let result = mac.finalize().into_bytes();

            if hex::encode(result) == code {
                let inner: RevolutWebhookBody = serde_json::from_slice(&msg.body)?;
                return Ok(inner);
            } else {
                warn!(
                    "Invalid signature found {} != {}",
                    code,
                    hex::encode(result)
                );
            }
        }

        bail!("No valid signature found!");
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RevolutWebhookEvent {
    OrderAuthorised,
    OrderCompleted,
    OrderCancelled,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    pub events: Vec<RevolutWebhookEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhook::WebhookMessage;
    use hmac::Mac;
    use std::collections::HashMap;

    fn create_revolut_signature(secret: &str, version: &str, timestamp: &str, body: &[u8]) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(version.as_bytes());
        mac.update(b".");
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let result = mac.finalize().into_bytes();
        format!("{}={}", version, hex::encode(result))
    }

    #[test]
    fn test_revolut_webhook_verify_valid() {
        let secret = "test_secret";
        let timestamp = "1234567890";
        let body = r#"{"event":"ORDER_COMPLETED","order_id":"order_123","merchant_order_ext_ref":null}"#;

        let signature = create_revolut_signature(secret, "v1", timestamp, body.as_bytes());

        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::from([
                ("revolut-signature".to_string(), signature),
                ("revolut-request-timestamp".to_string(), timestamp.to_string()),
            ]),
        };

        let result = RevolutWebhookBody::verify(secret, &msg);
        assert!(result.is_ok());
        let webhook = result.unwrap();
        assert_eq!(webhook.order_id, "order_123");
        assert!(matches!(webhook.event, RevolutWebhookEvent::OrderCompleted));
    }

    #[test]
    fn test_revolut_webhook_verify_missing_signature() {
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: b"{}".to_vec(),
            headers: HashMap::from([
                ("revolut-request-timestamp".to_string(), "1234567890".to_string()),
            ]),
        };

        let result = RevolutWebhookBody::verify("secret", &msg);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing Revolut-Signature"));
    }

    #[test]
    fn test_revolut_webhook_verify_missing_timestamp() {
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: b"{}".to_vec(),
            headers: HashMap::from([
                ("revolut-signature".to_string(), "v1=abc123".to_string()),
            ]),
        };

        let result = RevolutWebhookBody::verify("secret", &msg);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing Revolut-Request-Timestamp"));
    }

    #[test]
    fn test_revolut_webhook_verify_invalid_signature() {
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: r#"{"event":"ORDER_COMPLETED","order_id":"123"}"#.as_bytes().to_vec(),
            headers: HashMap::from([
                ("revolut-signature".to_string(), "v1=invalid_signature".to_string()),
                ("revolut-request-timestamp".to_string(), "1234567890".to_string()),
            ]),
        };

        let result = RevolutWebhookBody::verify("secret", &msg);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No valid signature found"));
    }

    #[test]
    fn test_revolut_webhook_event_serde() {
        let json = r#""ORDER_COMPLETED""#;
        let event: RevolutWebhookEvent = serde_json::from_str(json).unwrap();
        assert!(matches!(event, RevolutWebhookEvent::OrderCompleted));

        let serialized = serde_json::to_string(&RevolutWebhookEvent::OrderAuthorised).unwrap();
        assert_eq!(serialized, r#""ORDER_AUTHORISED""#);
    }

    #[test]
    fn test_revolut_order_state_serde() {
        let json = r#""completed""#;
        let state: RevolutOrderState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, RevolutOrderState::Completed));
    }

    #[test]
    fn test_revolut_payment_state_serde() {
        let json = r#""captured""#;
        let state: RevolutPaymentState = serde_json::from_str(json).unwrap();
        assert!(matches!(state, RevolutPaymentState::Captured));
    }

    #[test]
    fn test_revolut_line_item_simple() {
        let item = RevolutLineItem::simple("Test Item".to_string(), 2, 1000);
        assert_eq!(item.name, "Test Item");
        assert_eq!(item.quantity.value, 2);
        assert_eq!(item.unit_price_amount, 1000);
        assert_eq!(item.total_amount, 2000);
    }

    #[test]
    fn test_revolut_line_item_with_type() {
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100)
            .with_type(RevolutLineItemType::Digital);
        assert!(matches!(item.item_type, Some(RevolutLineItemType::Digital)));
    }

    #[test]
    fn test_revolut_line_item_with_description() {
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100)
            .with_description("A test item".to_string());
        assert_eq!(item.description, Some("A test item".to_string()));
    }

    #[test]
    fn test_revolut_line_item_with_unit() {
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100)
            .with_unit("kg".to_string());
        assert_eq!(item.quantity.unit, Some("kg".to_string()));
    }

    #[test]
    fn test_revolut_line_item_with_discounts() {
        let item = RevolutLineItem::simple("Test".to_string(), 2, 100)
            .with_discounts(vec![RevolutDiscount {
                name: "10% off".to_string(),
                amount: 20,
            }]);
        assert_eq!(item.total_amount, 180); // 200 - 20
    }

    #[test]
    fn test_revolut_line_item_with_taxes() {
        let item = RevolutLineItem::simple("Test".to_string(), 2, 100)
            .with_taxes(vec![RevolutTax {
                name: "VAT".to_string(),
                amount: 40,
            }]);
        assert_eq!(item.total_amount, 240); // 200 + 40
    }

    #[test]
    fn test_revolut_line_item_with_images() {
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100)
            .with_images(vec!["https://example.com/image.jpg".to_string()]);
        assert_eq!(
            item.image_urls,
            Some(vec!["https://example.com/image.jpg".to_string()])
        );
    }

    #[test]
    fn test_revolut_line_item_with_url() {
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100)
            .with_url("https://example.com/product".to_string());
        assert_eq!(item.url, Some("https://example.com/product".to_string()));
    }

    #[test]
    fn test_revolut_line_item_with_external_id() {
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100)
            .with_external_id("ext_123".to_string());
        assert_eq!(item.external_id, Some("ext_123".to_string()));
    }

    #[test]
    fn test_revolut_config_clone() {
        let config = RevolutConfig {
            url: Some("https://merchant.revolut.com".to_string()),
            api_version: "2024-09-01".to_string(),
            token: "test_token".to_string(),
            public_key: "pk_test".to_string(),
        };
        let cloned = config.clone();
        assert_eq!(cloned.token, "test_token");
        assert_eq!(cloned.api_version, "2024-09-01");
    }
}
