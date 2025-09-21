use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
};
use shd::{
    dex::pool_data::{calculate_pool_prices, get_pool_info, get_pools_batch, get_token_metadata},
    types::BotConfig,
};
use std::str::FromStr;
use tracing_subscriber::EnvFilter;

#[tokio::test]
async fn test_pool_data() {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .with_thread_ids(false)
        .with_line_number(false)
        .init();

    // Load configuration from TOML file
    let config_content = std::fs::read_to_string("config/main.toml").expect("Failed to read config file");
    let config: BotConfig = toml::from_str(&config_content).expect("Failed to parse config");

    // Create provider
    let provider = ProviderBuilder::new().on_http(config.global.rpc_endpoint.parse().expect("Invalid RPC URL"));

    tracing::info!("=== Pool Prices ({}/{}) ===", config.targets[0].base_token, config.targets[0].quote_token);
    
    for target in &config.targets {
        // Process Hyperswap pools
        if !target.hyperswap_pools.is_empty() {
            for pool_addr_str in &target.hyperswap_pools {
                if pool_addr_str.is_empty() {
                    continue;
                }
                
                if let Ok(pool_addr) = Address::from_str(pool_addr_str) {
                    process_pool(provider.clone(), pool_addr, "Hyperswap").await;
                }
            }
        }

        // Process ProjectX pools
        if !target.prjx_pools.is_empty() {
            for pool_addr_str in &target.prjx_pools {
                if pool_addr_str.is_empty() {
                    continue;
                }
                
                if let Ok(pool_addr) = Address::from_str(pool_addr_str) {
                    process_pool(provider.clone(), pool_addr, "ProjectX").await;
                }
            }
        }
    }

    // Test batch fetching and price comparison
    let mut all_pool_addresses: Vec<Address> = Vec::new();
    
    for target in &config.targets {
        for pool_str in &target.hyperswap_pools {
            if !pool_str.is_empty() {
                if let Ok(addr) = Address::from_str(pool_str) {
                    all_pool_addresses.push(addr);
                }
            }
        }
        for pool_str in &target.prjx_pools {
            if !pool_str.is_empty() {
                if let Ok(addr) = Address::from_str(pool_str) {
                    all_pool_addresses.push(addr);
                }
            }
        }
    }

    if all_pool_addresses.len() > 1 {
        match get_pools_batch(provider.clone(), all_pool_addresses).await {
            Ok(pools) => {
                let prices: Vec<_> = pools.iter().map(calculate_pool_prices).collect();
                
                // Find max spread
                let mut max_spread = 0.0;
                for i in 0..prices.len() {
                    for j in i + 1..prices.len() {
                        let price_diff_pct = ((prices[i].token0_price - prices[j].token0_price) / prices[i].token0_price).abs() * 100.0;
                        if price_diff_pct > max_spread {
                            max_spread = price_diff_pct;
                        }
                    }
                }
                
                if max_spread > 0.1 {
                    tracing::info!("\nMax spread: {:.3}%", max_spread);
                }
            }
            Err(_) => {}
        }
    }
}

async fn process_pool<P: Provider + Clone>(provider: P, pool_address: Address, dex_name: &str) {
    match get_pool_info(provider.clone(), pool_address).await {
        Ok(pool_info) => {
            // Get token metadata
            let token0_meta = get_token_metadata(provider.clone(), pool_info.token0).await.ok();
            let token1_meta = get_token_metadata(provider.clone(), pool_info.token1).await.ok();
            
            let token0_symbol = token0_meta.as_ref().map(|m| m.symbol.as_str()).unwrap_or("Token0");
            let token1_symbol = token1_meta.as_ref().map(|m| m.symbol.as_str()).unwrap_or("Token1");
            
            // Calculate prices
            let prices = calculate_pool_prices(&pool_info);
            
            // Display concise information
            let short_addr = format!("0x{}...{}", 
                &pool_address.to_string()[2..6], 
                &pool_address.to_string()[pool_address.to_string().len()-4..]);
            
            let liquidity_str = format_number(pool_info.liquidity.to::<u128>() as f64);
            
            // Display price (assuming token1 is USD-based)
            if token1_symbol.contains("USD") || token1_symbol.contains("USDT") || token1_symbol.contains("USDC") {
                tracing::info!(
                    "{} | {} | ${:.2}",
                    short_addr, liquidity_str, prices.token0_price
                );
            } else {
                tracing::info!(
                    "{} | {} | {:.6}",
                    short_addr, liquidity_str, prices.token0_price
                );
            }
        }
        Err(e) => {
            let short_addr = format!("0x{}...{}", 
                &pool_address.to_string()[2..6], 
                &pool_address.to_string()[pool_address.to_string().len()-4..]);
            tracing::warn!("{} | Failed: {}", short_addr, e);
        }
    }
}

fn format_number(n: f64) -> String {
    if n >= 1_000_000_000.0 {
        format!("{:.1}B", n / 1_000_000_000.0)
    } else if n >= 1_000_000.0 {
        format!("{:.1}M", n / 1_000_000.0)
    } else if n >= 1_000.0 {
        format!("{:.1}K", n / 1_000.0)
    } else {
        format!("{:.0}", n)
    }
}