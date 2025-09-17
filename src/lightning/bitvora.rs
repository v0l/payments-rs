use crate::json_api::JsonApi;
use crate::lightning::{AddInvoiceRequest, AddInvoiceResult, InvoiceUpdate, LightningNode};
use crate::webhook::{WEBHOOK_BRIDGE, WebhookMessage};
use anyhow::{anyhow, bail};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use hex::ToHex;
use hmac::{Hmac, Mac};
use lightning_invoice::Bolt11Invoice;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Clone)]
pub struct BitvoraNode {
    api: JsonApi,
    webhook_secret: String,
    /// Path used in the request for webhook matching
    webhook_path: String,
}

impl BitvoraNode {
    pub fn new(api_token: &str, webhook_secret: &str, webhook_path: &str) -> Self {
        let auth = format!("Bearer {}", api_token);
        Self {
            api: JsonApi::token("https://api.bitvora.com/", &auth, false).unwrap(),
            webhook_secret: webhook_secret.to_string(),
            webhook_path: webhook_path.to_string(),
        }
    }
}

#[async_trait]
impl LightningNode for BitvoraNode {
    async fn add_invoice(&self, req: AddInvoiceRequest) -> anyhow::Result<AddInvoiceResult> {
        let req = CreateInvoiceRequest {
            amount: req.amount / 1000,
            currency: "sats".to_string(),
            description: req.memo.unwrap_or_default(),
            expiry_seconds: req.expire.unwrap_or(3600) as u64,
        };
        let rsp: BitvoraResponse<CreateInvoiceResponse> = self
            .api
            .post("/v1/bitcoin/deposit/lightning-invoice", req)
            .await?;
        if rsp.status >= 400 {
            bail!(
                "API error: {} {}",
                rsp.status,
                rsp.message.unwrap_or_default()
            );
        }
        Ok(AddInvoiceResult {
            pr: rsp.data.payment_request,
            payment_hash: rsp.data.r_hash,
            external_id: Some(rsp.data.id),
        })
    }

    async fn cancel_invoice(&self, _id: &Vec<u8>) -> anyhow::Result<()> {
        bail!("Not supported yet!")
    }

    async fn subscribe_invoices(
        &self,
        _from_payment_hash: Option<Vec<u8>>,
    ) -> anyhow::Result<Pin<Box<dyn Stream<Item = InvoiceUpdate> + Send>>> {
        let rx = BroadcastStream::new(WEBHOOK_BRIDGE.listen());
        let secret = self.webhook_secret.clone();
        let webhook_path = self.webhook_path.clone();
        let mapped = rx.filter_map(move |r| {
            let secret = secret.clone();
            let webhook_path = webhook_path.clone();
            async move {
                match r {
                    Ok(r) => {
                        if r.endpoint != webhook_path {
                            // not being handled here, could be some other webhook event
                            return None;
                        }
                        let r_body = r.body.as_slice();
                        info!("Received webhook {}", String::from_utf8_lossy(r_body));
                        let body: BitvoraWebhook = match serde_json::from_slice(r_body) {
                            Ok(b) => b,
                            Err(e) => return Some(InvoiceUpdate::Error(e.to_string())),
                        };

                        if let Err(e) = verify_webhook(&secret, &r) {
                            return Some(InvoiceUpdate::Error(e.to_string()));
                        }

                        Some(match body.event {
                            BitvoraWebhookEvent::DepositLightningComplete => {
                                match body.data.recipient.parse::<Bolt11Invoice>() {
                                    Ok(invoice) => InvoiceUpdate::Settled {
                                        payment_hash: invoice.payment_hash().encode_hex(),
                                        preimage: None,
                                        external_id: Some(body.data.lightning_invoice_id),
                                    },
                                    Err(e) => InvoiceUpdate::Error(format!(
                                        "Failed to parse invoice: {}",
                                        e
                                    )),
                                }
                            }
                            BitvoraWebhookEvent::DepositLightningFailed => {
                                InvoiceUpdate::Error("Payment failed".to_string())
                            }
                        })
                    }
                    Err(e) => {
                        warn!("Error handling webhook: {}", e);
                        Some(InvoiceUpdate::Error(e.to_string()))
                    }
                }
            }
        });
        Ok(Box::pin(mapped))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CreateInvoiceRequest {
    pub amount: u64,
    pub currency: String,
    pub description: String,
    pub expiry_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct BitvoraResponse<T> {
    pub status: isize,
    pub message: Option<String>,
    pub data: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CreateInvoiceResponse {
    pub id: String,
    pub r_hash: String,
    pub payment_request: String,
}

#[derive(Deserialize, Debug, Clone)]
struct BitvoraWebhook {
    pub event: BitvoraWebhookEvent,
    pub data: BitvoraPayment,
}

#[derive(Deserialize, Debug, Clone)]
enum BitvoraWebhookEvent {
    #[serde(rename = "deposit.lightning.completed")]
    DepositLightningComplete,
    #[serde(rename = "deposit.lightning.failed")]
    DepositLightningFailed,
}

#[derive(Deserialize, Debug, Clone)]
struct BitvoraPayment {
    pub id: String,
    pub lightning_invoice_id: String,
    // the payment request
    pub recipient: String,
}

type HmacSha256 = Hmac<sha2::Sha256>;
fn verify_webhook(secret: &str, msg: &WebhookMessage) -> anyhow::Result<()> {
    let sig = msg
        .headers
        .get("bitvora-signature")
        .ok_or_else(|| anyhow!("Missing bitvora-signature header"))?;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())?;
    mac.update(msg.body.as_slice());
    let result = mac.finalize().into_bytes();

    if hex::encode(result) == *sig {
        return Ok(());
    } else {
        warn!("Invalid signature found {} != {}", sig, hex::encode(result));
    }

    bail!("No valid signature found!");
}
