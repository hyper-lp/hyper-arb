use alloy::{
    primitives::{Address, TxHash, U256},
    providers::Provider,
    sol,
};
use eyre::Result;
use std::str::FromStr;

use crate::{
    types::{ArbTarget, BotConfig, EnvConfig, PriceReference},
};

// Constants
const SWAP_GAS_UNITS: u128 = 150_000;
const SLIPPAGE_PERCENT: u64 = 5; // 5% slippage protection

/// Best arbitrage opportunity data
#[derive(Debug, Clone)]
pub struct BestOpportunity {
    pub dex: String,
    pub pool_address: String,
    pub pool_price: f64,
    pub spread_bps: f64,
    pub fee_bps: f64,
    pub net_profit_bps: f64,
    pub pool_fee_tier: u32,
}

/// Get current gas price from the network
async fn get_gas_price<P: Provider>(provider: P) -> Result<u128> {
    let gas_price = provider.get_gas_price().await?;
    Ok(gas_price)
}

// ===== ROUTER ABIs =====

// HyperSwap Router (7 params, no deadline) - Uniswap V3 style
sol! {
    #[sol(rpc)]
    IHyperSwapRouter,
    r#"[{
        "inputs": [{
            "components": [
                {"name": "tokenIn", "type": "address"},
                {"name": "tokenOut", "type": "address"},
                {"name": "fee", "type": "uint24"},
                {"name": "recipient", "type": "address"},
                {"name": "amountIn", "type": "uint256"},
                {"name": "amountOutMinimum", "type": "uint256"},
                {"name": "sqrtPriceLimitX96", "type": "uint160"}
            ],
            "name": "params",
            "type": "tuple"
        }],
        "name": "exactInputSingle",
        "outputs": [{"name": "amountOut", "type": "uint256"}],
        "stateMutability": "payable",
        "type": "function"
    }]"#
}

// ProjectX Router (8 params, with deadline)
sol! {
    #[sol(rpc)]
    IProjectXRouter,
    r#"[{
        "inputs": [{
            "components": [
                {"name": "tokenIn", "type": "address"},
                {"name": "tokenOut", "type": "address"},
                {"name": "fee", "type": "uint24"},
                {"name": "recipient", "type": "address"},
                {"name": "deadline", "type": "uint256"},
                {"name": "amountIn", "type": "uint256"},
                {"name": "amountOutMinimum", "type": "uint256"},
                {"name": "sqrtPriceLimitX96", "type": "uint160"}
            ],
            "name": "params",
            "type": "tuple"
        }],
        "name": "exactInputSingle",
        "outputs": [{"name": "amountOut", "type": "uint256"}],
        "stateMutability": "payable",
        "type": "function"
    }]"#
}

// ERC20 ABI for balance and allowance checks
sol! {
    #[sol(rpc)]
    IERC20,
    r#"[
        {
            "inputs": [{"name": "owner", "type": "address"}],
            "name": "balanceOf",
            "outputs": [{"name": "", "type": "uint256"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [
                {"name": "owner", "type": "address"},
                {"name": "spender", "type": "address"}
            ],
            "name": "allowance",
            "outputs": [{"name": "", "type": "uint256"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "decimals",
            "outputs": [{"name": "", "type": "uint8"}],
            "stateMutability": "view",
            "type": "function"
        }
    ]"#
}

/// Fetch price based on configured oracle reference
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

/// Execute statistical arbitrage trade
pub async fn execute_statistical_arbitrage<P: Provider + Clone>(
    provider: P,
    best_opportunity: BestOpportunity,
    target: &ArbTarget,
    env: &EnvConfig,
    config: &BotConfig,
    reference_price: f64,
) -> Result<()> {
    let BestOpportunity {
        dex,
        pool_address: pool_address_str,
        pool_price,
        spread_bps,
        fee_bps: _fee_bps,
        net_profit_bps,
        pool_fee_tier,
    } = best_opportunity;
    
    // Step 1: Gas price check
    let gas_price_wei = get_gas_price(provider.clone()).await?;
    let gas_price_gwei = gas_price_wei / 1_000_000_000;
    
    if gas_price_gwei > config.gas.max_gas_price_gwei as u128 {
        tracing::info!("⛽ Gas too high: {} gwei > {} max. Skipping.", 
            gas_price_gwei, config.gas.max_gas_price_gwei);
        return Ok(());
    }
    
    // Step 2: Get HYPE price safely (no fallback)
    let hype_price = match fetch_price_by_reference(&target.reference, "HYPE", config).await {
        Ok(price) if price > 0.0 => price,
        Ok(_) => {
            tracing::error!("Invalid HYPE price (0 or negative). Skipping trade.");
            return Ok(());
        },
        Err(e) => {
            tracing::error!("Failed to fetch HYPE price: {}. Skipping trade.", e);
            return Ok(());
        }
    };
    
    // Step 3: Calculate gas cost
    let gas_cost_wei = SWAP_GAS_UNITS * gas_price_wei;
    let gas_cost_hype = gas_cost_wei as f64 / 1e18;
    let gas_cost_usd = gas_cost_hype * hype_price;
    
    // Step 4: Get wallet and balances
    let wallet = match env.get_signer_for_address(&target.address) {
        Some(signer) => signer,
        None => {
            tracing::error!("No wallet found for target address: {}", target.address);
            return Ok(());
        }
    };
    let wallet_address = wallet.address();
    
    // Parse token addresses
    let base_token_address = Address::from_str(&target.base_token_address)?;
    let quote_token_address = Address::from_str(&target.quote_token_address)?;
    
    // Get token contracts for balance and decimal checks
    let base_token_contract = IERC20::new(base_token_address, provider.clone());
    let quote_token_contract = IERC20::new(quote_token_address, provider.clone());
    
    // Fetch decimals dynamically
    let base_decimals = base_token_contract.decimals().call().await?;
    let quote_decimals = quote_token_contract.decimals().call().await?;
    
    // Fetch balances
    let base_balance = base_token_contract.balanceOf(wallet_address).call().await?;
    let quote_balance = quote_token_contract.balanceOf(wallet_address).call().await?;
    
    // Step 5: Determine trade direction and calculate amount
    let (is_buy, token_in, token_out, balance_raw, decimals_in, decimals_out) = 
        if spread_bps < 0.0 {
            // Buy base with quote (pool cheaper than reference)
            (true, quote_token_address, base_token_address, 
             quote_balance, quote_decimals, base_decimals)
        } else {
            // Sell base for quote (pool more expensive than reference)
            (false, base_token_address, quote_token_address, 
             base_balance, base_decimals, quote_decimals)
        };
    
    // Calculate trade amount (inventory ratio)
    let amount_in_raw = (balance_raw.to::<u128>() as f64 * target.max_inventory_ratio) as u128;
    let amount_in = U256::from(amount_in_raw);
    
    // Step 6: Check minimum trade value in USD
    let amount_in_normalized = amount_in_raw as f64 / 10f64.powi(decimals_in as i32);
    let trade_value_usd = if is_buy {
        amount_in_normalized  // Already in USDT
    } else {
        amount_in_normalized * reference_price  // WHYPE to USD
    };
    
    if trade_value_usd < target.min_trade_value_usd {
        tracing::info!("Trade value ${:.2} below minimum ${:.2}. Skipping.", 
            trade_value_usd, target.min_trade_value_usd);
        return Ok(());
    }
    
    // Step 7: Check allowance (skip trade if insufficient)
    let router_address = match dex.to_lowercase().as_str() {
        "hyperswap" => {
            let router_str = config.dex.iter()
                .find(|d| d.name.to_lowercase() == "hyperswap")
                .map(|d| &d.router)
                .ok_or_else(|| eyre::eyre!("Hyperswap router not found in config"))?;
            Address::from_str(router_str)?
        },
        "projectx" => {
            let router_str = config.dex.iter()
                .find(|d| d.name.to_lowercase() == "projectx")
                .map(|d| &d.router)
                .ok_or_else(|| eyre::eyre!("ProjectX router not found in config"))?;
            Address::from_str(router_str)?
        },
        _ => return Err(eyre::eyre!("Unknown DEX: {}", dex)),
    };
    
    let token_in_contract = IERC20::new(token_in, provider.clone());
    let current_allowance = token_in_contract
        .allowance(wallet_address, router_address)
        .call()
        .await?;
    
    if current_allowance < amount_in {
        tracing::warn!("Insufficient allowance: {} < {}. Skipping trade.", 
            current_allowance, amount_in);
        tracing::info!("Set infinite approval with: cast send {} 'approve(address,uint256)' {} {}",
            token_in, router_address, U256::MAX);
        return Ok(());
    }
    
    // Step 8: Calculate expected output with slippage
    let expected_output = if is_buy {
        // Buying WHYPE with USDT: amount / price
        let output = amount_in_normalized / pool_price;
        U256::from((output * 10f64.powi(decimals_out as i32)) as u128)
    } else {
        // Selling WHYPE for USDT: amount * price
        let output = amount_in_normalized * pool_price;
        U256::from((output * 10f64.powi(decimals_out as i32)) as u128)
    };
    
    let amount_out_min = expected_output * U256::from(100 - SLIPPAGE_PERCENT) / U256::from(100);
    
    // Step 9: Log trade details
    tracing::info!("📊 Executing {} on {}:", 
        if is_buy { "BUY" } else { "SELL" }, dex);
    tracing::info!("  Pool: {} | Fee tier: {}", &pool_address_str[..10], pool_fee_tier);
    tracing::info!("  Amount in: {:.6} ({:.1}% of balance)", 
        amount_in_normalized, target.max_inventory_ratio * 100.0);
    tracing::info!("  Value: ${:.2} | Gas: ${:.2} | Net profit: {:.2} bps",
        trade_value_usd, gas_cost_usd, 
        net_profit_bps - (gas_cost_usd / trade_value_usd * 10000.0));
    
    // Step 10: Check if we're in testing mode
    if env.testing {
        tracing::info!("🧪 TESTING MODE - Trade would be executed but not broadcast");
        tracing::info!("  Would send swap to {} router: {}", dex, router_address);
        tracing::info!("  Token in: {} | Token out: {}", token_in, token_out);
        tracing::info!("  Amount in: {} | Min out: {}", amount_in, amount_out_min);
        return Ok(());
    }
    
    // Step 11: Log RPC endpoint being used for broadcast
    if config.global.broadcast_rpc_endpoint.is_some() {
        tracing::debug!("Using broadcast RPC endpoint for swap transaction");
    }
    
    // Step 12: Build and execute swap based on DEX
    let tx_hash = match dex.to_lowercase().as_str() {
        "hyperswap" => {
            execute_hyperswap(
                provider.clone(),
                router_address,
                token_in,
                token_out,
                amount_in,
                amount_out_min,
                pool_fee_tier,
                wallet_address,
                gas_price_wei,
                config,
                wallet.clone(),
            ).await?
        },
        "projectx" => {
            execute_projectx(
                provider.clone(),
                router_address,
                token_in,
                token_out,
                amount_in,
                amount_out_min,
                pool_fee_tier,
                wallet_address,
                gas_price_wei,
                config,
                wallet.clone(),
            ).await?
        },
        _ => return Err(eyre::eyre!("Unknown DEX")),
    };
    
    tracing::info!("✅ Swap executed: 0x{:x}", tx_hash);
    tracing::info!("   Explorer: {}tx/0x{:x}", config.global.explorer_base_url, tx_hash);
    Ok(())
}

/// Execute swap on Hyperswap (7 params, no deadline)
async fn execute_hyperswap<P: Provider + Clone>(
    _provider: P,
    router_address: Address,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    amount_out_min: U256,
    fee: u32,
    recipient: Address,
    gas_price: u128,
    config: &BotConfig,
    wallet: alloy::signers::local::PrivateKeySigner,
) -> Result<TxHash> {
    use alloy::network::EthereumWallet;
    use alloy::providers::ProviderBuilder;
    use std::sync::Arc;
    
    // Build wallet provider with signer (use broadcast RPC if available)
    let rpc_url = config.global.broadcast_rpc_endpoint
        .as_ref()
        .unwrap_or(&config.global.rpc_endpoint);
    let eth_wallet = EthereumWallet::from(wallet);
    let provider = ProviderBuilder::new()
        .wallet(eth_wallet)
        .connect_http(rpc_url.parse()?);
    
    // Create router contract instance
    let router = IHyperSwapRouter::new(router_address, Arc::new(provider));
    
    // Prepare swap call
    let gas_limit = (SWAP_GAS_UNITS as f64 * config.gas.gas_estimate_multiplier) as u64;
    let adjusted_gas_price = (gas_price as f64 * config.gas.gas_price_multiplier) as u128;
    
    // Execute swap with contract method (will handle signing)
    let pending = router
        .exactInputSingle((
            token_in,
            token_out,
            alloy::primitives::Uint::<24, 1>::from(fee),
            recipient,
            amount_in,
            amount_out_min,
            alloy::primitives::Uint::<160, 3>::ZERO, // sqrtPriceLimitX96
        ))
        .gas(gas_limit)
        .gas_price(adjusted_gas_price)
        .send()
        .await?;
    
    let tx_hash = *pending.tx_hash();
    let _receipt = pending.get_receipt().await?;
    
    Ok(tx_hash)
}

/// Execute swap on ProjectX (8 params, with deadline)
async fn execute_projectx<P: Provider + Clone>(
    _provider: P,
    router_address: Address,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    amount_out_min: U256,
    fee: u32,
    recipient: Address,
    gas_price: u128,
    config: &BotConfig,
    wallet: alloy::signers::local::PrivateKeySigner,
) -> Result<TxHash> {
    use alloy::network::EthereumWallet;
    use alloy::providers::ProviderBuilder;
    use std::sync::Arc;
    
    // Build wallet provider with signer (use broadcast RPC if available)
    let rpc_url = config.global.broadcast_rpc_endpoint
        .as_ref()
        .unwrap_or(&config.global.rpc_endpoint);
    let eth_wallet = EthereumWallet::from(wallet);
    let provider = ProviderBuilder::new()
        .wallet(eth_wallet)
        .connect_http(rpc_url.parse()?);
    
    // Create router contract instance
    let router = IProjectXRouter::new(router_address, Arc::new(provider));
    
    // ProjectX uses 8 params (with deadline)
    let deadline = U256::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() + 300 // 5 minutes
    );
    
    // Prepare swap call
    let gas_limit = (SWAP_GAS_UNITS as f64 * config.gas.gas_estimate_multiplier) as u64;
    let adjusted_gas_price = (gas_price as f64 * config.gas.gas_price_multiplier) as u128;
    
    // Execute swap with contract method (will handle signing)
    let pending = router
        .exactInputSingle((
            token_in,
            token_out,
            alloy::primitives::Uint::<24, 1>::from(fee),
            recipient,
            deadline,  // Extra param for ProjectX
            amount_in,
            amount_out_min,
            alloy::primitives::Uint::<160, 3>::ZERO,
        ))
        .gas(gas_limit)
        .gas_price(adjusted_gas_price)
        .send()
        .await?;
    
    let tx_hash = *pending.tx_hash();
    let _receipt = pending.get_receipt().await?;
    
    Ok(tx_hash)
}