use alloy::{
    primitives::{Address, U256},
    providers::Provider,
};
use eyre::Result;
use std::str::FromStr;

use crate::{
    types::{ArbTarget, BotConfig, EnvConfig, PriceReference},
};

use super::swap::{
    BestOpportunity, DoubleLegOpportunity, PoolSwapParams, SpotOrderParams,
    get_gas_price, SLIPPAGE_PERCENT, SWAP_GAS_UNITS, IERC20,
};

// Helper function to fetch price by reference
async fn fetch_price_by_reference(reference: &PriceReference, symbol: &str, config: &BotConfig) -> Result<f64> {
    match reference {
        PriceReference::Pyth => match symbol.to_uppercase().as_str() {
            "BTC" => crate::oracles::pyth::fetch_btc_usd_price().await,
            "ETH" => crate::oracles::pyth::fetch_eth_usd_price().await,
            "HYPE" | "WHYPE" => crate::oracles::fetch_hype_usd_price().await,
            _ => Err(eyre::eyre!("Pyth oracle doesn't support {} price", symbol)),
        },
        PriceReference::Redstone => match symbol.to_uppercase().as_str() {
            "BTC" => crate::oracles::fetch_btc_usd_price().await,
            "ETH" => crate::oracles::fetch_eth_usd_price().await,
            "HYPE" | "WHYPE" => crate::oracles::redstone::fetch_hype_usd_price().await,
            _ => {
                let redstone = crate::oracles::Redstone::new();
                redstone.get_price(symbol).await
            }
        },
        PriceReference::Hypercore => {
            let hypercore = crate::oracles::Hypercore::new(config);
            hypercore.get_price(symbol).await
        }
    }
}

/// Prepare double-leg arbitrage parameters without executing
/// Returns pool swap params for DEX leg and spot order params for CoreWriter leg
pub async fn prepare_double_leg_arbitrage<P: Provider + Clone>(
    provider: P,
    buy_opportunity: BestOpportunity,
    sell_opportunity: BestOpportunity,
    target: &ArbTarget,
    env: &EnvConfig,
    config: &BotConfig,
    reference_price: f64,
) -> Result<(PoolSwapParams, SpotOrderParams, DoubleLegOpportunity)> {
    // Step 1: Gas price check
    let gas_price_wei = get_gas_price(provider.clone()).await?;
    let gas_price_gwei = gas_price_wei / 1_000_000_000;
    
    if gas_price_gwei > config.gas.max_gas_price_gwei as u128 {
        return Err(eyre::eyre!("Gas too high: {} gwei > {} max", 
            gas_price_gwei, config.gas.max_gas_price_gwei));
    }
    
    // Step 2: Get HYPE price
    let hype_price = match fetch_price_by_reference(&target.reference, "HYPE", config).await {
        Ok(price) if price > 0.0 => price,
        Ok(_) => return Err(eyre::eyre!("Invalid HYPE price")),
        Err(e) => return Err(eyre::eyre!("Failed to fetch HYPE price: {}", e)),
    };
    
    // Step 3: Calculate gas cost for both legs
    let gas_cost_wei = SWAP_GAS_UNITS * 2 * gas_price_wei; // Double gas for two swaps
    let gas_cost_hype = gas_cost_wei as f64 / 1e18;
    let gas_cost_usd = gas_cost_hype * hype_price;
    
    // Step 4: Get wallet and balances
    let wallet = match env.get_signer_for_address(&target.address) {
        Some(signer) => signer,
        None => return Err(eyre::eyre!("No wallet found for target address: {}", target.address)),
    };
    let wallet_address = wallet.address();
    
    // Parse token addresses
    let base_token_address = Address::from_str(&target.base_token_address)?;
    let quote_token_address = Address::from_str(&target.quote_token_address)?;
    
    // Get token contracts
    let base_token_contract = IERC20::new(base_token_address, provider.clone());
    let quote_token_contract = IERC20::new(quote_token_address, provider.clone());
    
    // Fetch decimals
    let base_decimals = base_token_contract.decimals().call().await?;
    let quote_decimals = quote_token_contract.decimals().call().await?;
    
    // Fetch balances
    let base_balance = base_token_contract.balanceOf(wallet_address).call().await?;
    let quote_balance = quote_token_contract.balanceOf(wallet_address).call().await?;
    
    // Step 5: Calculate optimal trade amounts
    // For buy leg: spending quote tokens (USDT)
    let max_quote_spend = (quote_balance.to::<u128>() as f64 * target.max_inventory_ratio) as u128;
    
    // For sell leg: selling base tokens (WHYPE) 
    let max_base_sell = (base_balance.to::<u128>() as f64 * target.max_inventory_ratio) as u128;
    
    // Calculate how much WHYPE we can buy with our quote tokens
    let quote_spend_normalized = max_quote_spend as f64 / 10f64.powi(quote_decimals as i32);
    let expected_base_from_buy = quote_spend_normalized / buy_opportunity.pool_price;
    let expected_base_raw = (expected_base_from_buy * 10f64.powi(base_decimals as i32)) as u128;
    
    // Take the minimum between what we can buy and what we can sell
    let base_amount_to_trade = expected_base_raw.min(max_base_sell);
    
    // Recalculate quote amount based on final base amount
    let base_normalized = base_amount_to_trade as f64 / 10f64.powi(base_decimals as i32);
    let quote_amount_for_buy = (base_normalized * buy_opportunity.pool_price * 10f64.powi(quote_decimals as i32)) as u128;
    
    let amount_in_buy = U256::from(quote_amount_for_buy);
    let amount_in_sell = U256::from(base_amount_to_trade);
    
    // Step 6: Check minimum trade value
    let trade_value_usd = base_normalized * reference_price;
    if trade_value_usd < target.min_trade_value_usd {
        return Err(eyre::eyre!("Trade value ${:.2} below minimum ${:.2}", 
            trade_value_usd, target.min_trade_value_usd));
    }
    
    // Step 7: Calculate expected profit
    let buy_cost = base_normalized * buy_opportunity.pool_price;
    let sell_revenue = base_normalized * sell_opportunity.pool_price;
    let expected_profit_usd = sell_revenue - buy_cost - gas_cost_usd;
    
    if expected_profit_usd <= 0.0 {
        return Err(eyre::eyre!("No profit after gas costs: ${:.2}", expected_profit_usd));
    }
    
    // Step 8: Calculate slippage-adjusted outputs
    // For buy leg: expected WHYPE output
    let expected_base_out = amount_in_buy.to::<u128>() as f64 / 10f64.powi(quote_decimals as i32) / buy_opportunity.pool_price;
    let expected_base_out_raw = (expected_base_out * 10f64.powi(base_decimals as i32)) as u128;
    let min_base_out = U256::from(expected_base_out_raw) * U256::from(100 - SLIPPAGE_PERCENT) / U256::from(100);
    
    // For sell leg: expected USDT output  
    let expected_quote_out = amount_in_sell.to::<u128>() as f64 / 10f64.powi(base_decimals as i32) * sell_opportunity.pool_price;
    let expected_quote_out_raw = (expected_quote_out * 10f64.powi(quote_decimals as i32)) as u128;
    let _min_quote_out = U256::from(expected_quote_out_raw) * U256::from(100 - SLIPPAGE_PERCENT) / U256::from(100);
    
    // Step 9: Get router addresses
    let buy_router = match buy_opportunity.dex.to_lowercase().as_str() {
        "hyperswap" => Address::from_str(&config.dex.iter()
            .find(|d| d.name.to_lowercase() == "hyperswap")
            .ok_or_else(|| eyre::eyre!("Hyperswap router not found"))?.router)?,
        "projectx" => Address::from_str(&config.dex.iter()
            .find(|d| d.name.to_lowercase() == "projectx")
            .ok_or_else(|| eyre::eyre!("ProjectX router not found"))?.router)?,
        _ => return Err(eyre::eyre!("Unknown DEX: {}", buy_opportunity.dex)),
    };
    
    // Step 10: Prepare pool swap params for buy leg
    let pool_swap_params = PoolSwapParams {
        dex: buy_opportunity.dex.clone(),
        router_address: buy_router,
        token_in: quote_token_address,
        token_out: base_token_address,
        amount_in: amount_in_buy,
        amount_out_min: min_base_out,
        pool_address: buy_opportunity.pool_address.clone(),
        pool_fee_tier: buy_opportunity.pool_fee_tier,
        recipient: wallet_address,
    };
    
    // Step 11: Prepare spot order params for sell leg (CoreWriter)
    let spot_order_params = SpotOrderParams {
        base_token: target.base_token.clone(),
        quote_token: target.quote_token.clone(),
        is_buy: false, // Selling base for quote
        amount: base_normalized,
        price: sell_opportunity.pool_price,
        slippage: SLIPPAGE_PERCENT as f64 / 100.0,
    };
    
    // Step 12: Create double leg opportunity
    let double_leg_opportunity = DoubleLegOpportunity {
        buy_leg: buy_opportunity,
        sell_leg: sell_opportunity,
        amount_in_buy,
        amount_in_sell,
        expected_profit_usd,
        gas_cost_usd,
    };
    
    // Step 13: Log preparation details
    tracing::info!("📊 Double-leg arbitrage prepared:");
    tracing::info!("  Buy on {} at ${:.4} | Sell on {} at ${:.4}", 
        pool_swap_params.dex, double_leg_opportunity.buy_leg.pool_price,
        double_leg_opportunity.sell_leg.dex, double_leg_opportunity.sell_leg.pool_price);
    tracing::info!("  Trade size: {:.6} {} (${:.2})", 
        base_normalized, target.base_token, trade_value_usd);
    tracing::info!("  Expected profit: ${:.2} | Gas cost: ${:.2}", 
        expected_profit_usd, gas_cost_usd);
    tracing::info!("  Pool swap: {} -> {} via {}", 
        target.quote_token, target.base_token, pool_swap_params.dex);
    tracing::info!("  Spot order: Sell {} {} at ${:.4} on CoreWriter", 
        spot_order_params.amount, spot_order_params.base_token, spot_order_params.price);
    
    Ok((pool_swap_params, spot_order_params, double_leg_opportunity))
}