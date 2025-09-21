use eyre::Result;
use serde_json::json;

#[tokio::test]
async fn test_spot_meta_raw() -> Result<()> {
    println!("\nFetching raw spotMeta response from Hyperliquid API...\n");
    
    let client = reqwest::Client::new();
    let api_url = "https://api.hyperliquid.xyz/info";
    
    let payload = json!({
        "type": "spotMeta"
    });
    
    let response = client
        .post(api_url)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;
    
    if !response.status().is_success() {
        println!("Error: HTTP {}", response.status());
        let text = response.text().await?;
        println!("Response: {}", text);
        return Ok(());
    }
    
    let json_value: serde_json::Value = response.json().await?;
    
    // Pretty print the raw JSON
    println!("Raw API Response:");
    println!("{}", serde_json::to_string_pretty(&json_value)?);
    
    // Check the structure
    if let Some(obj) = json_value.as_object() {
        println!("\nTop-level keys: {:?}", obj.keys().collect::<Vec<_>>());
        
        // Check if it's directly an object with token keys
        if let Some(first_entry) = obj.values().next() {
            println!("\nFirst entry structure:");
            println!("{}", serde_json::to_string_pretty(&first_entry)?);
        }
    } else if let Some(arr) = json_value.as_array() {
        println!("\nResponse is an array with {} elements", arr.len());
        if let Some(first) = arr.first() {
            println!("\nFirst element:");
            println!("{}", serde_json::to_string_pretty(&first)?);
        }
    }
    
    Ok(())
}