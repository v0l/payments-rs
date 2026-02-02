use std::env::args;
use anyhow::Result;
use payments_rs::currency::{Currency, CurrencyAmount};
use payments_rs::fiat::{
    LineItem, RevolutApi, RevolutConfig, RevolutDiscount, RevolutLineItem, RevolutLineItemType,
    RevolutTax,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize the logger
    env_logger::init();

    // Create Revolut API client configuration
    let config = RevolutConfig {
        url: Some("https://merchant.revolut.com".to_string()),
        api_version: "2024-09-01".to_string(),
        token: args().nth(1).unwrap(),
        public_key: "your_public_key".to_string(),
    };

    // Create the Revolut API client
    let revolut = RevolutApi::new(config)?;

    // Example 1: Create a simple order (no line items)
    println!("Creating a simple order...");
    let amount = CurrencyAmount::from_f32(Currency::GBP, 20.00);
    let order = revolut
        .create_order(amount, Some("Simple test order".to_string()), None)
        .await?;
    println!("Order created: {:?}", order);
    println!("Checkout URL: {:?}", order.checkout_url);

    // Example 2: Create an order with line items using the generic LineItem
    println!("\nCreating an order with generic line items...");
    let line_items = vec![
        LineItem {
            name: "Premium Widget".to_string(),
            description: Some("A high-quality widget".to_string()),
            unit_amount: 2500, // £25.00 in pence
            quantity: 2,
            currency: "gbp".to_string(),
            images: Some(vec!["https://example.com/widget.jpg".to_string()]),
            metadata: None,
            tax_amount: Some(1000), // £10.00 VAT (20% on £50)
            tax_name: Some("20% VAT".to_string()),
        },
        LineItem {
            name: "Standard Gadget".to_string(),
            description: Some("An everyday gadget".to_string()),
            unit_amount: 1000, // £10.00 in pence
            quantity: 1,
            currency: "gbp".to_string(),
            images: None,
            metadata: None,
            tax_amount: Some(200), // £2.00 VAT (20% on £10)
            tax_name: Some("20% VAT".to_string()),
        },
    ];

    let total_amount = line_items.iter().map(|i| i.total_amount()).sum::<u64>();
    let subtotal = line_items.iter().map(|i| i.subtotal_amount()).sum::<u64>();
    let tax_total = total_amount - subtotal;
    
    println!("Subtotal: £{}.{:02}", subtotal / 100, subtotal % 100);
    println!("Tax: £{}.{:02}", tax_total / 100, tax_total % 100);
    println!("Total: £{}.{:02}", total_amount / 100, total_amount % 100);
    
    let amount_with_items = CurrencyAmount::from_u64(Currency::GBP, total_amount);

    let order_with_items = revolut
        .create_order(
            amount_with_items,
            Some("Order with line items".to_string()),
            Some(line_items),
        )
        .await?;
    println!("Order with line items: {:?}", order_with_items);

    // Example 3: Use the FiatPaymentService trait (simple order)
    println!("\nCreating another simple order...");
    let amount = CurrencyAmount::from_f32(Currency::GBP, 50.00);
    let simple_order = revolut
        .create_order(amount, Some("Order #12345".to_string()), None)
        .await?;
    println!("Simple order created: {:?}", simple_order);

    // Example 4: Cancel an order
    println!("\nCancelling the order...");
    revolut.cancel_order(&simple_order.id).await?;
    println!("Order cancelled successfully");

    // Example 5: List webhooks
    println!("\nListing webhooks...");
    let webhooks = revolut.list_webhooks().await?;
    println!("Webhooks: {:?}", webhooks);

    // Example 6: Advanced line items with Revolut-specific features
    // Note: To use advanced features like discounts and taxes, you would need to 
    // create a wrapper method in RevolutApi or use the builder pattern directly
    println!("\nAdvanced Revolut line items example:");
    println!("You can create line items with discounts, taxes, and more using RevolutLineItem::simple()");
    
    let advanced_item = RevolutLineItem::simple("Example item".to_string(), 2, 100)
        .with_type(RevolutLineItemType::Physical)
        .with_unit("kg".to_string())
        .with_description("First line item".to_string())
        .with_discounts(vec![RevolutDiscount {
            name: "Discount 1".to_string(),
            amount: 50,
        }])
        .with_taxes(vec![RevolutTax {
            name: "10% VAT".to_string(),
            amount: 20,
        }])
        .with_images(vec!["https://www.example.com/image.jpg".to_string()])
        .with_url("https://www.example.com".to_string())
        .with_external_id("external_id_123".to_string());

    println!("Advanced line item created: {:?}", advanced_item);
    println!("Total amount (with discounts and taxes): {}", advanced_item.total_amount);

    Ok(())
}
