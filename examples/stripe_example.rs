use std::env::args;
use anyhow::Result;
use payments_rs::currency::{Currency, CurrencyAmount};
use payments_rs::fiat::{
    CheckoutLineItem, CreateCheckoutSessionRequest, FiatPaymentService, LineItem, PriceData,
    ProductData, StripeApi, StripeConfig,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the logger
    env_logger::init();

    // Create Stripe API client configuration
    let config = StripeConfig {
        url: Some("https://api.stripe.com".to_string()),
        api_key: args().nth(1).unwrap(),
        webhook_secret: Some("your_webhook_secret".to_string()),
    };

    // Create the Stripe API client
    let stripe = StripeApi::new(config)?;

    // Example 1: Create a Payment Intent
    println!("Creating a payment intent...");
    let amount = CurrencyAmount::from_f32(Currency::USD, 20.00);
    let payment_intent = stripe
        .create_payment_intent(amount, Some("Test payment".to_string()))
        .await?;
    println!("Payment Intent created: {:?}", payment_intent);
    println!("Client Secret: {:?}", payment_intent.client_secret);

    // Example 2: Retrieve a Payment Intent
    println!("\nRetrieving the payment intent...");
    let retrieved = stripe.get_payment_intent(&payment_intent.id).await?;
    println!("Retrieved Payment Intent: {:?}", retrieved);

    // Example 3: Create a Checkout Session
    println!("\nCreating a checkout session...");
    let checkout_request = CreateCheckoutSessionRequest {
        line_items: vec![CheckoutLineItem {
            price_data: Some(PriceData {
                currency: "usd".to_string(),
                unit_amount: 2000, // $20.00 in cents
                product_data: ProductData {
                    name: "Test Product".to_string(),
                    description: Some("A test product".to_string()),
                    images: None,
                    metadata: None,
                },
                recurring: None,
                tax_behavior: None,
            }),
            price: None,
            quantity: 1,
            tax_rates: None,
        }],
        mode: "payment".to_string(),
        success_url: Some("https://example.com/success".to_string()),
        cancel_url: Some("https://example.com/cancel".to_string()),
        customer_email: Some("customer@example.com".to_string()),
        customer: None,
        client_reference_id: Some("order_123".to_string()),
        metadata: None,
        expires_at: None,
    };

    let checkout_session = stripe.create_checkout_session(checkout_request).await?;
    println!("Checkout Session created: {:?}", checkout_session);
    println!("Checkout URL: {:?}", checkout_session.url);

    // Example 3b: List checkout sessions
    println!("\nListing checkout sessions...");
    let sessions = stripe.list_checkout_sessions(Some(10)).await?;
    println!("Found {} sessions", sessions.data.len());

    // Example 3c: Get line items
    println!("\nRetrieving line items...");
    let line_items = stripe
        .get_checkout_session_line_items(&checkout_session.id)
        .await?;
    println!("Line items: {:?}", line_items);

    // Example 4: Set up Webhooks
    println!("\nListing existing webhooks...");
    let webhooks = stripe.list_webhooks().await?;
    println!("Existing webhooks: {:?}", webhooks);

    // Create a new webhook
    println!("\nCreating a webhook...");
    let webhook = stripe
        .create_webhook(
            "https://your-domain.com/webhook",
            vec![
                "payment_intent.succeeded".to_string(),
                "payment_intent.payment_failed".to_string(),
                "checkout.session.completed".to_string(),
            ],
        )
        .await?;
    println!("Webhook created: {:?}", webhook);
    println!("Webhook Secret: {:?}", webhook.secret);

    // Example 5: Use the FiatPaymentService trait
    println!("\nUsing FiatPaymentService trait...");
    let amount = CurrencyAmount::from_f32(Currency::USD, 50.00); // $50.00
    let payment_info = stripe.create_order("Order #12345", amount, None).await?;
    println!("Payment Info: {:?}", payment_info);

    // Example 6: Use FiatPaymentService trait with line items
    println!("\nUsing FiatPaymentService trait with line items...");
    let line_items = vec![
        LineItem {
            name: "Premium Widget".to_string(),
            description: Some("A high-quality widget".to_string()),
            unit_amount: 2500, // $25.00 in cents
            quantity: 2,
            currency: "usd".to_string(),
            images: Some(vec!["https://example.com/widget.jpg".to_string()]),
            metadata: None,
            tax_amount: Some(500), // $5.00 tax (10% VAT on $50)
            tax_name: Some("10% VAT".to_string()),
        },
        LineItem {
            name: "Standard Gadget".to_string(),
            description: Some("An everyday gadget".to_string()),
            unit_amount: 1000, // $10.00 in cents
            quantity: 1,
            currency: "usd".to_string(),
            images: None,
            metadata: None,
            tax_amount: Some(100), // $1.00 tax (10% VAT on $10)
            tax_name: Some("10% VAT".to_string()),
        },
    ];
    
    let total_amount = line_items.iter().map(|i| i.total_amount()).sum::<u64>();
    let amount_with_items = CurrencyAmount::from_u64(Currency::USD, total_amount);
    
    println!("Line items total (including tax): ${}.{:02}", total_amount / 100, total_amount % 100);
    
    let payment_with_items = stripe
        .create_order("Order #12346 with line items", amount_with_items, Some(line_items))
        .await?;
    println!("Payment with line items: {:?}", payment_with_items);

    // Example 7: Cancel a payment intent
    println!("\nCancelling the payment intent...");
    stripe.cancel_order(&payment_info.external_id).await?;
    println!("Payment intent cancelled successfully");

    Ok(())
}
