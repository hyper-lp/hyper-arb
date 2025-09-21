use reqwest;
use serde_json::Value;

#[tokio::test]
async fn debug_hyperliquid_api() {
    let client = reqwest::Client::new();
    
    // Test 1: Get meta
    println!("\n=== Testing meta endpoint ===");
    let payload = serde_json::json!({
        "type": "meta"
    });
    
    let resp = client
        .post("https://api.hyperliquid.xyz/info")
        .json(&payload)
        .send()
        .await
        .unwrap();
    
    let text = resp.text().await.unwrap();
    let json: Value = serde_json::from_str(&text).unwrap();
    
    // Pretty print first few chars
    let pretty = serde_json::to_string_pretty(&json).unwrap();
    println!("Meta response structure:");
    println!("{}", &pretty[..pretty.len().min(500)]);
    
    // Test 2: Get allMids
    println!("\n=== Testing allMids endpoint ===");
    let payload = serde_json::json!({
        "type": "allMids"
    });
    
    let resp = client
        .post("https://api.hyperliquid.xyz/info")
        .json(&payload)
        .send()
        .await
        .unwrap();
    
    let text = resp.text().await.unwrap();
    let json: Value = serde_json::from_str(&text).unwrap();
    
    let pretty = serde_json::to_string_pretty(&json).unwrap();
    println!("AllMids response structure:");
    println!("{}", &pretty[..pretty.len().min(1000)]);
    
    // Check for specific prices
    if let Some(obj) = json.as_object() {
        println!("\nTop-level keys: {:?}", obj.keys().collect::<Vec<_>>());
        
        // Look for BTC, ETH, HYPE
        for symbol in ["BTC", "ETH", "HYPE"] {
            if let Some(price) = obj.get(symbol) {
                println!("{}: {}", symbol, price);
            }
        }
    }
    
    // Test 3: Try array access if it's an array
    if let Some(arr) = json.as_array() {
        println!("\nResponse is array with {} elements", arr.len());
        if !arr.is_empty() {
            println!("First element: {:?}", arr[0]);
        }
    }
}