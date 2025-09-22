use alloy::providers::{Provider, ProviderBuilder};
use alloy::primitives::Address;
use shd::dex::pool_data::{get_pool_info, calculate_pool_prices, get_token_metadata};
use shd::types::load_bot_config_with_env;
use std::str::FromStr;

#[tokio::test]
async fn debug_eth_pools() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::from_filename("config/.env").ok();
    let env = shd::types::EnvConfig::new();
    let config = load_bot_config_with_env("config/main.toml", &env);
    
    let provider = ProviderBuilder::new()
        .connect_http(config.global.rpc_endpoint.parse()?);
    
    println!("\n=== DEBUGGING ETH POOL PRICES ===\n");
    
    // ETH pools from charlie target
    let pools = vec![
        ("Hyperswap", "0x2850fe0dcf4ca5e0a7b8355f4a875f96a92de948"),
        ("ProjectX", "0xaEAE69783e3121196A45f3930fa141f462A4Df2F"),
    ];
    
    for (dex, pool_addr_str) in pools {
        println!("DEX: {} - Pool: {}", dex, pool_addr_str);
        
        let pool_addr = Address::from_str(pool_addr_str)?;
        let pool_info = get_pool_info(provider.clone(), pool_addr).await?;
        
        // Get token metadata
        let token0_meta = get_token_metadata(provider.clone(), pool_info.token0).await?;
        let token1_meta = get_token_metadata(provider.clone(), pool_info.token1).await?;
        
        println!("  Token0: {} ({}) - Address: {}", 
            token0_meta.symbol, token0_meta.decimals, pool_info.token0);
        println!("  Token1: {} ({}) - Address: {}", 
            token1_meta.symbol, token1_meta.decimals, pool_info.token1);
        
        let prices = calculate_pool_prices(&pool_info);
        println!("  Price token0/token1: ${:.6}", prices.token0_price);
        println!("  Price token1/token0: ${:.6}", prices.token1_price);
        
        // Determine which token is ETH and calculate the correct price
        if token0_meta.symbol.contains("ETH") || token0_meta.symbol == "WETH" {
            println!("  ETH is token0 -> ETH/USDT price: ${:.2}", prices.token0_price);
        } else if token1_meta.symbol.contains("ETH") || token1_meta.symbol == "WETH" {
            println!("  ETH is token1 -> ETH/USDT price: ${:.2}", prices.token1_price);
        }
        
        println!("  Tick: {}", pool_info.tick);
        println!("  Liquidity: {}", pool_info.liquidity);
        println!("  Fee: {} bps\n", pool_info.fee as f64 / 100.0);
    }
    
    Ok(())
}