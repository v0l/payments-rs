use crate::currency::{Currency, CurrencyAmount};
use crate::fiat::{FiatPaymentInfo, FiatPaymentService, LineItem, SubscriptionPaymentInfo};
use crate::json_api::{JsonApi, TokenGen};
use crate::webhook::{WebhookMessage, verify_timestamp_within};
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
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
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

    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn create_order(
        &self,
        amount: CurrencyAmount,
        description: Option<String>,
        line_items: Option<Vec<LineItem>>,
    ) -> Result<RevolutOrder> {
        self.create_order_ext(amount, description, line_items, None, None)
            .await
    }

    /// Create an order with optional saved-payment-method support.
    ///
    /// * `customer` - Optional customer to create/attach to the order. Required
    ///   (with at least an email, or an existing `id`) when saving a payment
    ///   method or charging a saved one.
    /// * `save_payment_method_for` - When set (e.g. `"merchant"`), instructs
    ///   Revolut to save the payment method used to complete this order for
    ///   future off-session, merchant-initiated charges.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn create_order_ext(
        &self,
        amount: CurrencyAmount,
        description: Option<String>,
        line_items: Option<Vec<LineItem>>,
        customer: Option<RevolutCustomer>,
        save_payment_method_for: Option<String>,
    ) -> Result<RevolutOrder> {
        // Convert generic LineItems to Revolut's format
        let revolut_line_items = line_items.map(|items| {
            items
                .into_iter()
                .map(|item| {
                    let total = item.total_amount();

                    // Build taxes array if tax info is provided
                    let taxes =
                        if let (Some(tax_amt), Some(tax_name)) = (item.tax_amount, item.tax_name) {
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
                    customer,
                    save_payment_method_for,
                },
            )
            .await
    }

    /// Pay for an existing order using a customer's saved payment method.
    ///
    /// This drives the merchant-initiated (off-session) charge once a payment
    /// method has been saved for the merchant. See
    /// [`RevolutApi::create_off_session_order`] for the full flow.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn pay_order(
        &self,
        order_id: &str,
        req: PayOrderRequest,
    ) -> Result<RevolutOrderPayment> {
        self.api
            .post(&format!("/api/orders/{}/payments", order_id), req)
            .await
    }

    /// Create and charge an order off-session against a saved payment method
    /// (merchant-initiated), returning the resulting order once charged.
    ///
    /// This is a two-step flow: create an order attached to the customer, then
    /// pay for it using the saved payment method with `initiator = merchant`.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub async fn create_off_session_order(
        &self,
        customer_id: &str,
        payment_method_id: &str,
        payment_method_type: RevolutSavedPaymentMethodType,
        amount: CurrencyAmount,
        description: Option<String>,
    ) -> Result<RevolutOrder> {
        let order = self
            .create_order_ext(
                amount,
                description,
                None,
                Some(RevolutCustomer {
                    id: Some(customer_id.to_string()),
                    ..Default::default()
                }),
                None,
            )
            .await?;
        self.pay_order(
            &order.id,
            PayOrderRequest {
                saved_payment_method: SavedPaymentMethodRef {
                    kind: payment_method_type.as_str().to_string(),
                    id: payment_method_id.to_string(),
                    initiator: "merchant".to_string(),
                },
            },
        )
        .await?;
        // Re-fetch to return the final order state after the charge
        self.get_order(&order.id).await
    }

    pub async fn get_order(&self, order_id: &str) -> Result<RevolutOrder> {
        self.api.get(&format!("/api/orders/{}", order_id)).await
    }

    /// Retrieve a customer's saved payment methods.
    ///
    /// The reusable payment method id (needed for off-session/merchant-initiated
    /// charges) is NOT returned on the order object — it must be fetched here
    /// after the customer completes a savable checkout.
    ///
    /// When `only_merchant` is true, only payment methods saved for
    /// merchant-initiated transactions (`saved_for == "merchant"`) are returned
    /// (filtered client-side).
    ///
    /// Uses the `/api/1.0/customers/{id}/payment-methods` endpoint, which
    /// returns a bare JSON array.
    pub async fn get_customer_payment_methods(
        &self,
        customer_id: &str,
        only_merchant: bool,
    ) -> Result<Vec<RevolutSavedPaymentMethod>> {
        let all: Vec<RevolutSavedPaymentMethod> = self
            .api
            .get(&format!(
                "/api/1.0/customers/{}/payment-methods",
                customer_id
            ))
            .await?;
        Ok(if only_merchant {
            all.into_iter()
                .filter(|pm| pm.is_merchant_initiated())
                .collect()
        } else {
            all
        })
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

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn create_subscription(
        &self,
        description: &str,
        amount: CurrencyAmount,
        customer_email: Option<String>,
        line_items: Option<Vec<LineItem>>,
    ) -> Pin<Box<dyn Future<Output = Result<SubscriptionPaymentInfo>> + Send>> {
        let s = self.clone();
        let desc = description.to_string();
        Box::pin(async move {
            let customer = customer_email.map(|email| RevolutCustomer {
                email: Some(email),
                ..Default::default()
            });
            let rsp = s
                .create_order_ext(
                    amount,
                    Some(desc),
                    line_items,
                    customer,
                    Some("merchant".to_string()),
                )
                .await?;
            // Note: the reusable saved payment_method_id is not available yet at
            // order-creation time — it only exists once the customer completes
            // the savable checkout. Fetch it later via
            // `get_customer_payment_methods(customer_id, true)`.
            Ok(SubscriptionPaymentInfo {
                external_id: rsp.id.clone(),
                customer_id: rsp.customer_id(),
                payment_method_id: None,
                checkout_url: rsp.checkout_url.clone(),
                raw_data: serde_json::to_string(&rsp)?,
            })
        })
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    fn charge_subscription(
        &self,
        customer_id: &str,
        payment_method_id: &str,
        amount: CurrencyAmount,
        description: &str,
    ) -> Pin<Box<dyn Future<Output = Result<FiatPaymentInfo>> + Send>> {
        let s = self.clone();
        let customer_id = customer_id.to_string();
        let payment_method_id = payment_method_id.to_string();
        let desc = description.to_string();
        Box::pin(async move {
            let rsp = s
                .create_off_session_order(
                    &customer_id,
                    &payment_method_id,
                    RevolutSavedPaymentMethodType::Card,
                    amount,
                    Some(desc),
                )
                .await?;
            Ok(FiatPaymentInfo {
                raw_data: serde_json::to_string(&rsp)?,
                external_id: rsp.id,
            })
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

    /// Customer to create/attach to the order. Required for saving or charging
    /// a saved payment method.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<RevolutCustomer>,

    /// When set (e.g. `"merchant"`), save the payment method used to complete
    /// this order for future off-session charges.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_payment_method_for: Option<String>,
}

/// A customer to create or attach to an order.
///
/// Provide an existing `id` to attach a known customer, or an `email` (and
/// optionally `phone` / `full_name`) to create a new one. At least an email is
/// required in order to save a payment method.
#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct RevolutCustomer {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
}

/// Request body to pay for an order using a customer's saved payment method.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PayOrderRequest {
    pub saved_payment_method: SavedPaymentMethodRef,
}

/// Reference to a saved payment method used when charging off-session.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SavedPaymentMethodRef {
    /// Payment method type as returned at the customer level (`card` or
    /// `revolut_pay`).
    #[serde(rename = "type")]
    pub kind: String,
    /// Saved payment method ID.
    pub id: String,
    /// Who initiates the payment: `customer` or `merchant`. Off-session
    /// recurring charges must use `merchant`.
    pub initiator: String,
}

/// Saved payment method type used when charging a saved method off-session.
///
/// At the customer level Revolut only exposes `card` and `revolut_pay`.
///
/// The Revolut API returns these in SCREAMING_SNAKE_CASE (`CARD`,
/// `REVOLUT_PAY`) on the customer payment-methods endpoint, but accepts/returns
/// lowercase elsewhere — accept both on deserialize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevolutSavedPaymentMethodType {
    #[serde(rename = "CARD", alias = "card")]
    Card,
    #[serde(rename = "REVOLUT_PAY", alias = "revolut_pay")]
    RevolutPay,
}

impl RevolutSavedPaymentMethodType {
    /// The wire value used in the Revolut API.
    pub fn as_str(&self) -> &'static str {
        match self {
            RevolutSavedPaymentMethodType::Card => "card",
            RevolutSavedPaymentMethodType::RevolutPay => "revolut_pay",
        }
    }
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
            total_amount: quantity.saturating_mul(unit_price_amount),
            external_id: None,
            discounts: None,
            taxes: None,
            image_urls: None,
            url: None,
        }
    }

    /// Recompute `total_amount` from the current quantity, unit price, discounts
    /// and taxes.
    ///
    /// `total = quantity * unit_price - sum(discounts) + sum(taxes)`, computed
    /// with saturating arithmetic. This keeps the total consistent regardless of
    /// the order in which the builder methods are applied.
    fn recalculate_total(&mut self) {
        let base = self.quantity.value.saturating_mul(self.unit_price_amount);
        let discount_total: u64 = self
            .discounts
            .as_ref()
            .map(|d| d.iter().map(|x| x.amount).sum())
            .unwrap_or(0);
        let tax_total: u64 = self
            .taxes
            .as_ref()
            .map(|t| t.iter().map(|x| x.amount).sum())
            .unwrap_or(0);
        self.total_amount = base
            .saturating_sub(discount_total)
            .saturating_add(tax_total);
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

    /// Builder-style method to add discounts.
    ///
    /// The total is recomputed from all components, so this is safe to combine
    /// with [`RevolutLineItem::with_taxes`] in any order.
    pub fn with_discounts(mut self, discounts: Vec<RevolutDiscount>) -> Self {
        self.discounts = Some(discounts);
        self.recalculate_total();
        self
    }

    /// Builder-style method to add taxes.
    ///
    /// The total is recomputed from all components, so this is safe to combine
    /// with [`RevolutLineItem::with_discounts`] in any order.
    pub fn with_taxes(mut self, taxes: Vec<RevolutTax>) -> Self {
        self.taxes = Some(taxes);
        self.recalculate_total();
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
    /// Customer attached to the order (present once a customer is
    /// created/attached, e.g. after a savable checkout completes). The Revolut
    /// API nests this as an object (`customer.id`), NOT a top-level
    /// `customer_id` — use [`RevolutOrder::customer_id`] to read the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customer: Option<RevolutOrderCustomer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payments: Option<Vec<RevolutOrderPayment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_items: Option<Vec<RevolutLineItem>>,
}

impl RevolutOrder {
    /// The id of the customer attached to this order, if any.
    pub fn customer_id(&self) -> Option<String> {
        self.customer.as_ref().map(|c| c.id.clone())
    }

    /// Extract the saved payment method (id and type) from the order's
    /// payments, if a payment method was saved during checkout.
    ///
    /// Note: the Revolut order object does NOT carry the reusable saved payment
    /// method id — use [`RevolutApi::get_customer_payment_methods`] with the
    /// customer id instead. This helper only inspects the inline payment method.
    pub fn saved_payment_method(&self) -> Option<(String, RevolutPaymentMethodType)> {
        self.payments.as_ref()?.iter().find_map(|p| {
            let pm = p.payment_method.as_ref()?;
            let id = pm.id.as_ref()?;
            Some((id.clone(), pm.kind.clone()))
        })
    }
}

/// Customer object nested on a [`RevolutOrder`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutOrderCustomer {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A payment method saved against a Revolut customer, returned by
/// [`RevolutApi::get_customer_payment_methods`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutSavedPaymentMethod {
    /// Reusable payment method id used for off-session charges.
    pub id: String,
    /// Payment method type (`card` or `revolut_pay`).
    #[serde(rename = "type")]
    pub kind: RevolutSavedPaymentMethodType,
    /// Who the method was saved for: `customer` or `merchant`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_for: Option<String>,
    /// Card details (present for `card` methods): brand, last 4, expiry, etc.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method_details: Option<RevolutSavedCardDetails>,
}

/// Non-sensitive card details for a saved payment method (PCI-safe: no PAN/CVV).
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutSavedCardDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last4: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry_month: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry_year: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub funding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer_country: Option<String>,
}

impl RevolutSavedPaymentMethod {
    /// Whether this saved method supports merchant-initiated (off-session)
    /// transactions. The API returns `saved_for` as `MERCHANT`/`CUSTOMER`
    /// (case-insensitive match).
    pub fn is_merchant_initiated(&self) -> bool {
        self.saved_for
            .as_deref()
            .map(|s| s.eq_ignore_ascii_case("merchant"))
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RevolutOrderPayment {
    pub id: String,
    pub state: RevolutPaymentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decline_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bank_message: Option<String>,
    // The pay-order (off-session charge) response omits these timestamps, so
    // they are optional even though the order fetch includes them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
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
    /// Default tolerance for webhook timestamp replay protection (5 minutes).
    pub const DEFAULT_TOLERANCE: std::time::Duration = std::time::Duration::from_secs(300);

    /// Verify and parse a Revolut webhook event.
    ///
    /// This checks the HMAC signature in constant time and rejects events whose
    /// timestamp is outside [`RevolutWebhookBody::DEFAULT_TOLERANCE`] of the
    /// current time (replay protection). Use
    /// [`RevolutWebhookBody::verify_with_tolerance`] to customise or disable the
    /// timestamp check.
    pub fn verify(secret: &str, msg: &WebhookMessage) -> Result<Self> {
        Self::verify_with_tolerance(secret, msg, Some(Self::DEFAULT_TOLERANCE))
    }

    /// Verify and parse a Revolut webhook event with a configurable timestamp
    /// tolerance.
    ///
    /// Pass `Some(tolerance)` to enable replay protection, or `None` to skip the
    /// timestamp check entirely (not recommended in production).
    pub fn verify_with_tolerance(
        secret: &str,
        msg: &WebhookMessage,
        tolerance: Option<std::time::Duration>,
    ) -> Result<Self> {
        let sig = msg
            .headers
            .get("revolut-signature")
            .ok_or_else(|| anyhow!("Missing Revolut-Signature header"))?;
        let timestamp = msg
            .headers
            .get("revolut-request-timestamp")
            .ok_or_else(|| anyhow!("Missing Revolut-Request-Timestamp header"))?;

        // check if any signatures match (constant-time comparison)
        let mut verified = false;
        for sig in sig.split(",") {
            let mut sig_split = sig.split("=");
            let (version, code) = (
                sig_split.next().context("Invalid signature format")?,
                sig_split.next().context("Invalid signature format")?,
            );
            let Ok(expected) = hex::decode(code) else {
                warn!("Invalid signature encoding: {}", code);
                continue;
            };
            // HMAC accepts keys of any length, so `new_from_slice` cannot fail.
            let mut mac =
                HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
            mac.update(version.as_bytes());
            mac.update(b".");
            mac.update(timestamp.as_bytes());
            mac.update(b".");
            mac.update(msg.body.as_slice());
            if mac.verify_slice(&expected).is_ok() {
                verified = true;
                break;
            }
            warn!("Invalid signature found for version {}", version);
        }

        if !verified {
            bail!("No valid signature found!");
        }

        // Replay protection: the Revolut timestamp is in milliseconds since the
        // epoch.
        if let Some(tolerance) = tolerance {
            let ts_ms: i64 = timestamp
                .parse()
                .context("Invalid Revolut-Request-Timestamp header")?;
            verify_timestamp_within(ts_ms / 1000, tolerance)?;
        }

        let inner: RevolutWebhookBody = serde_json::from_slice(&msg.body)?;
        Ok(inner)
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

    fn create_revolut_signature(
        secret: &str,
        version: &str,
        timestamp: &str,
        body: &[u8],
    ) -> String {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(version.as_bytes());
        mac.update(b".");
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let result = mac.finalize().into_bytes();
        format!("{}={}", version, hex::encode(result))
    }

    fn now_millis() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[test]
    fn test_revolut_webhook_verify_valid() {
        let secret = "test_secret";
        let timestamp = now_millis().to_string();
        let body =
            r#"{"event":"ORDER_COMPLETED","order_id":"order_123","merchant_order_ext_ref":null}"#;

        let signature = create_revolut_signature(secret, "v1", &timestamp, body.as_bytes());

        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::from([
                ("revolut-signature".to_string(), signature),
                ("revolut-request-timestamp".to_string(), timestamp),
            ]),
        };

        let result = RevolutWebhookBody::verify(secret, &msg);
        assert!(result.is_ok());
        let webhook = result.unwrap();
        assert_eq!(webhook.order_id, "order_123");
        assert!(matches!(webhook.event, RevolutWebhookEvent::OrderCompleted));
    }

    #[test]
    fn test_revolut_webhook_verify_expired_timestamp_rejected() {
        // Regression: a validly-signed but old event must be rejected by the
        // default replay-protection window.
        let secret = "test_secret";
        let timestamp = (now_millis() - 3_600_000).to_string();
        let body = r#"{"event":"ORDER_COMPLETED","order_id":"order_123"}"#;

        let signature = create_revolut_signature(secret, "v1", &timestamp, body.as_bytes());
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::from([
                ("revolut-signature".to_string(), signature),
                ("revolut-request-timestamp".to_string(), timestamp),
            ]),
        };

        let result = RevolutWebhookBody::verify(secret, &msg);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("outside tolerance")
        );
    }

    #[test]
    fn test_revolut_webhook_verify_no_tolerance_allows_old() {
        let secret = "test_secret";
        let timestamp = "1234567890";
        let body = r#"{"event":"ORDER_COMPLETED","order_id":"order_123"}"#;

        let signature = create_revolut_signature(secret, "v1", timestamp, body.as_bytes());
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: body.as_bytes().to_vec(),
            headers: HashMap::from([
                ("revolut-signature".to_string(), signature),
                (
                    "revolut-request-timestamp".to_string(),
                    timestamp.to_string(),
                ),
            ]),
        };

        let result = RevolutWebhookBody::verify_with_tolerance(secret, &msg, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_revolut_webhook_verify_missing_signature() {
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: b"{}".to_vec(),
            headers: HashMap::from([(
                "revolut-request-timestamp".to_string(),
                "1234567890".to_string(),
            )]),
        };

        let result = RevolutWebhookBody::verify("secret", &msg);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing Revolut-Signature")
        );
    }

    #[test]
    fn test_revolut_webhook_verify_missing_timestamp() {
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: b"{}".to_vec(),
            headers: HashMap::from([("revolut-signature".to_string(), "v1=abc123".to_string())]),
        };

        let result = RevolutWebhookBody::verify("secret", &msg);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Missing Revolut-Request-Timestamp")
        );
    }

    #[test]
    fn test_revolut_webhook_verify_invalid_signature() {
        let msg = WebhookMessage {
            endpoint: "/webhooks/revolut".to_string(),
            body: r#"{"event":"ORDER_COMPLETED","order_id":"123"}"#.as_bytes().to_vec(),
            headers: HashMap::from([
                (
                    "revolut-signature".to_string(),
                    "v1=invalid_signature".to_string(),
                ),
                (
                    "revolut-request-timestamp".to_string(),
                    "1234567890".to_string(),
                ),
            ]),
        };

        let result = RevolutWebhookBody::verify("secret", &msg);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No valid signature found")
        );
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
        let item = RevolutLineItem::simple("Test".to_string(), 1, 100).with_unit("kg".to_string());
        assert_eq!(item.quantity.unit, Some("kg".to_string()));
    }

    #[test]
    fn test_revolut_line_item_with_discounts() {
        let item = RevolutLineItem::simple("Test".to_string(), 2, 100).with_discounts(vec![
            RevolutDiscount {
                name: "10% off".to_string(),
                amount: 20,
            },
        ]);
        assert_eq!(item.total_amount, 180); // 200 - 20
    }

    #[test]
    fn test_revolut_line_item_with_taxes() {
        let item =
            RevolutLineItem::simple("Test".to_string(), 2, 100).with_taxes(vec![RevolutTax {
                name: "VAT".to_string(),
                amount: 40,
            }]);
        assert_eq!(item.total_amount, 240); // 200 + 40
    }

    #[test]
    fn test_revolut_line_item_discounts_and_taxes_order_independent() {
        // Regression: applying discounts then taxes (or vice versa) must yield
        // the same total. Previously `with_discounts` recomputed from the base
        // and discarded any taxes already applied.
        let discounts = vec![RevolutDiscount {
            name: "10% off".to_string(),
            amount: 20,
        }];
        let taxes = vec![RevolutTax {
            name: "VAT".to_string(),
            amount: 40,
        }];

        let a = RevolutLineItem::simple("Test".to_string(), 2, 100)
            .with_discounts(discounts.clone())
            .with_taxes(taxes.clone());
        let b = RevolutLineItem::simple("Test".to_string(), 2, 100)
            .with_taxes(taxes)
            .with_discounts(discounts);

        // 200 base - 20 discount + 40 tax = 220
        assert_eq!(a.total_amount, 220);
        assert_eq!(b.total_amount, 220);
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
    fn test_saved_payment_method_type_as_str() {
        assert_eq!(RevolutSavedPaymentMethodType::Card.as_str(), "card");
        assert_eq!(
            RevolutSavedPaymentMethodType::RevolutPay.as_str(),
            "revolut_pay"
        );
    }

    #[test]
    fn test_create_order_request_serialize_minimal() {
        // customer / save_payment_method_for omitted when None
        let req = CreateOrderRequest {
            amount: 1000,
            currency: "EUR".to_string(),
            description: Some("Test".to_string()),
            line_items: None,
            customer: None,
            save_payment_method_for: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("customer").is_none());
        assert!(json.get("save_payment_method_for").is_none());
        assert_eq!(json["amount"], 1000);
    }

    #[test]
    fn test_create_order_request_serialize_with_customer_and_save() {
        let req = CreateOrderRequest {
            amount: 2500,
            currency: "USD".to_string(),
            description: None,
            line_items: None,
            customer: Some(RevolutCustomer {
                email: Some("a@b.com".to_string()),
                ..Default::default()
            }),
            save_payment_method_for: Some("merchant".to_string()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["save_payment_method_for"], "merchant");
        assert_eq!(json["customer"]["email"], "a@b.com");
        // unset customer fields are skipped
        assert!(json["customer"].get("id").is_none());
        assert!(json["customer"].get("phone").is_none());
    }

    #[test]
    fn test_revolut_customer_default_serialize_empty() {
        let c = RevolutCustomer::default();
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn test_pay_order_request_serialize() {
        let req = PayOrderRequest {
            saved_payment_method: SavedPaymentMethodRef {
                kind: "card".to_string(),
                id: "pm_123".to_string(),
                initiator: "merchant".to_string(),
            },
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["saved_payment_method"]["type"], "card");
        assert_eq!(json["saved_payment_method"]["id"], "pm_123");
        assert_eq!(json["saved_payment_method"]["initiator"], "merchant");
    }

    #[test]
    fn test_order_deserialize_customer_id_and_saved_method() {
        let json = r#"{
            "id": "order_1",
            "token": "tok_1",
            "state": "completed",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "amount": 1000,
            "currency": "EUR",
            "outstanding_amount": 0,
            "customer": { "id": "cust_42", "email": "a@b.com" },
            "payments": [{
                "id": "pay_1",
                "state": "captured",
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z",
                "amount": 1000,
                "payment_method": {
                    "id": "pm_99",
                    "type": "card",
                    "card_last_four": "4242"
                }
            }]
        }"#;
        let order: RevolutOrder = serde_json::from_str(json).unwrap();
        assert_eq!(order.customer_id().as_deref(), Some("cust_42"));
        let (pm_id, pm_type) = order.saved_payment_method().unwrap();
        assert_eq!(pm_id, "pm_99");
        assert!(matches!(pm_type, RevolutPaymentMethodType::Card));
    }

    #[test]
    fn test_order_saved_payment_method_none_when_no_payments() {
        let json = r#"{
            "id": "order_1",
            "token": "tok_1",
            "state": "pending",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "amount": 1000,
            "currency": "EUR",
            "outstanding_amount": 1000
        }"#;
        let order: RevolutOrder = serde_json::from_str(json).unwrap();
        assert!(order.customer_id().is_none());
        assert!(order.saved_payment_method().is_none());
    }

    #[test]
    fn test_order_saved_payment_method_none_when_method_missing_id() {
        // A payment without a payment_method.id (not saved) yields None
        let json = r#"{
            "id": "order_1",
            "token": "tok_1",
            "state": "completed",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "amount": 1000,
            "currency": "EUR",
            "outstanding_amount": 0,
            "payments": [{
                "id": "pay_1",
                "state": "captured",
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z",
                "amount": 1000,
                "payment_method": { "type": "card" }
            }]
        }"#;
        let order: RevolutOrder = serde_json::from_str(json).unwrap();
        assert!(order.saved_payment_method().is_none());
    }

    #[test]
    fn test_saved_payment_method_deserialize_and_mit_filter() {
        // Uppercase form is what the live customer payment-methods endpoint
        // returns; lowercase is accepted via aliases for robustness.
        let json = r#"[
            { "id": "pm_merchant", "type": "CARD", "saved_for": "MERCHANT", "method_details": { "brand": "VISA", "last4": "5709", "expiry_month": 12, "expiry_year": 2029 } },
            { "id": "pm_customer", "type": "card", "saved_for": "customer" },
            { "id": "pm_none", "type": "revolut_pay" }
        ]"#;
        let methods: Vec<RevolutSavedPaymentMethod> = serde_json::from_str(json).unwrap();
        assert_eq!(methods.len(), 3);
        assert_eq!(methods[0].id, "pm_merchant");
        assert_eq!(
            methods[0]
                .method_details
                .as_ref()
                .and_then(|d| d.last4.as_deref()),
            Some("5709")
        );
        assert!(matches!(
            methods[0].kind,
            RevolutSavedPaymentMethodType::Card
        ));
        assert!(matches!(
            methods[2].kind,
            RevolutSavedPaymentMethodType::RevolutPay
        ));
        assert!(methods[0].is_merchant_initiated());
        assert!(!methods[1].is_merchant_initiated());
        assert!(!methods[2].is_merchant_initiated());
    }

    #[test]
    fn test_order_customer_nested_id() {
        let json = r#"{
            "id": "order_1",
            "token": "tok_1",
            "state": "completed",
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "amount": 1000,
            "currency": "EUR",
            "outstanding_amount": 0,
            "customer": { "id": "cust_nested" }
        }"#;
        let order: RevolutOrder = serde_json::from_str(json).unwrap();
        assert_eq!(order.customer_id().as_deref(), Some("cust_nested"));
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
