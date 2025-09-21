use shd::oracles::{pyth, redstone};

#[tokio::test]
async fn test_all_oracles() {
    println!("\n=== Pyth ===");
    match pyth::fetch_btc_usd_price().await {
        Ok(p) => println!("BTC: ${:.2}", p),
        Err(e) => println!("BTC: Error - {}", e),
    }
    match pyth::fetch_eth_usd_price().await {
        Ok(p) => println!("ETH: ${:.2}", p),
        Err(e) => println!("ETH: Error - {}", e),
    }
    match pyth::fetch_hype_usd_price().await {
        Ok(p) => println!("HYPE: ${:.2}", p),
        Err(e) => println!("HYPE: Error - {}", e),
    }

    println!("\n=== Redstone ===");
    match redstone::fetch_btc_usd_price().await {
        Ok(p) => println!("BTC: ${:.2}", p),
        Err(e) => println!("BTC: Error - {}", e),
    }
    match redstone::fetch_eth_usd_price().await {
        Ok(p) => println!("ETH: ${:.2}", p),
        Err(e) => println!("ETH: Error - {}", e),
    }
    match redstone::fetch_hype_usd_price().await {
        Ok(p) => println!("HYPE: ${:.2}", p),
        Err(e) => println!("HYPE: Error - {}", e),
    }

    println!("\n=== Hypercore (API) ===");
    match shd::core::api::fetch_btc_price().await {
        Ok(p) => println!("BTC: ${:.2}", p),
        Err(e) => println!("BTC: Error - {}", e),
    }
    match shd::core::api::fetch_eth_price().await {
        Ok(p) => println!("ETH: ${:.2}", p),
        Err(e) => println!("ETH: Error - {}", e),
    }
    match shd::core::api::fetch_hype_price().await {
        Ok(p) => println!("HYPE: ${:.2}", p),
        Err(e) => println!("HYPE: Error - {}", e),
    }
}