use alloy::{
    network::Network,
    primitives::{Address, U256},
    providers::{Provider, RootProvider},
    rpc::types::Filter,
};
use eyre::Result;
use serde::{Deserialize, Serialize};

// ===== POOL DATA STRUCTURES =====

/// Pool information from DEX
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolInfo {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub liquidity: U256,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub token0_decimals: u8,
    pub token1_decimals: u8,
}

/// Pool price data
#[derive(Debug, Clone)]
pub struct PoolPrice {
    pub pool_address: Address,
    pub token0_price: f64, // Price of token0 in terms of token1
    pub token1_price: f64, // Price of token1 in terms of token0
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub fee: u32, // Pool fee in basis points (e.g., 500 = 0.05%)
    pub liquidity: U256, // Pool liquidity for depth analysis
}

/// Token metadata
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenMetadata {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub address: Address,
}

/// Pool TVL (Total Value Locked) data
#[derive(Debug, Clone)]
pub struct PoolTVL {
    pub pool_address: Address,
    pub token0_amount: f64,
    pub token1_amount: f64,
    pub total_value_usd: Option<f64>,
}

// ===== UNISWAP V3 POOL ABI =====

// Minimal ABI for Uniswap V3 Pool
alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IUniswapV3Pool,
    r#"[
        {
            "inputs": [],
            "name": "slot0",
            "outputs": [
                {"internalType": "uint160", "name": "sqrtPriceX96", "type": "uint160"},
                {"internalType": "int24", "name": "tick", "type": "int24"},
                {"internalType": "uint16", "name": "observationIndex", "type": "uint16"},
                {"internalType": "uint16", "name": "observationCardinality", "type": "uint16"},
                {"internalType": "uint16", "name": "observationCardinalityNext", "type": "uint16"},
                {"internalType": "uint8", "name": "feeProtocol", "type": "uint8"},
                {"internalType": "bool", "name": "unlocked", "type": "bool"}
            ],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "liquidity",
            "outputs": [{"internalType": "uint128", "name": "", "type": "uint128"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "token0",
            "outputs": [{"internalType": "address", "name": "", "type": "address"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "token1",
            "outputs": [{"internalType": "address", "name": "", "type": "address"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "fee",
            "outputs": [{"internalType": "uint24", "name": "", "type": "uint24"}],
            "stateMutability": "view",
            "type": "function"
        }
    ]"#
);

// ERC20 Interface for token metadata
alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IERC20Metadata,
    r#"[
        {
            "inputs": [],
            "name": "name",
            "outputs": [{"internalType": "string", "name": "", "type": "string"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "symbol",
            "outputs": [{"internalType": "string", "name": "", "type": "string"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [],
            "name": "decimals",
            "outputs": [{"internalType": "uint8", "name": "", "type": "uint8"}],
            "stateMutability": "view",
            "type": "function"
        },
        {
            "inputs": [{"internalType": "address", "name": "account", "type": "address"}],
            "name": "balanceOf",
            "outputs": [{"internalType": "uint256", "name": "", "type": "uint256"}],
            "stateMutability": "view",
            "type": "function"
        }
    ]"#
);

// ===== POOL DATA EXTRACTION FUNCTIONS =====

/// Get pool information from Uniswap V3 pool
pub async fn get_pool_info<P: Provider + Clone>(provider: P, pool_address: Address) -> Result<PoolInfo> {
    let pool = IUniswapV3Pool::new(pool_address, provider.clone());

    // Get pool slot0 data
    let slot0 = pool.slot0().call().await?;
    let liquidity = pool.liquidity().call().await?;
    let token0 = pool.token0().call().await?;
    let token1 = pool.token1().call().await?;
    let fee = pool.fee().call().await?;

    // Get token decimals
    let token0_contract = IERC20Metadata::new(token0, provider.clone());
    let token1_contract = IERC20Metadata::new(token1, provider.clone());

    let token0_decimals = token0_contract.decimals().call().await?;
    let token1_decimals = token1_contract.decimals().call().await?;

    Ok(PoolInfo {
        address: pool_address,
        token0,
        token1,
        fee: fee.to::<u32>(),
        liquidity: U256::from(liquidity),
        sqrt_price_x96: U256::from(slot0.sqrtPriceX96),
        tick: slot0.tick.as_i32(),
        token0_decimals,
        token1_decimals,
    })
}

/// Calculate pool prices from sqrt price
pub fn calculate_pool_prices(pool_info: &PoolInfo) -> PoolPrice {
    // Convert sqrtPriceX96 to actual price
    // price = (sqrtPriceX96 / 2^96)^2 * (10^decimals0 / 10^decimals1)

    // Convert U256 to f64 safely
    let sqrt_price_f64 = if let Ok(as_u128) = u128::try_from(pool_info.sqrt_price_x96) {
        as_u128 as f64
    } else {
        // For very large numbers, use string conversion as fallback
        pool_info.sqrt_price_x96.to_string().parse::<f64>().unwrap_or(0.0)
    };

    // Normalize sqrt price
    let sqrt_price_normalized = sqrt_price_f64 / (2_f64.powf(96.0));
    let price_raw = sqrt_price_normalized.powi(2);

    // Apply decimal adjustment: price = P × 10^(decimals0 - decimals1)
    // This gives us: how much token1 per token0
    let decimal_adjustment = 10_f64.powf((pool_info.token0_decimals as f64) - (pool_info.token1_decimals as f64));
    let price_token1_per_token0 = price_raw * decimal_adjustment;

    // Calculate inverse price
    let price_token0_per_token1 = if price_token1_per_token0 != 0.0 && price_token1_per_token0.is_finite() {
        1.0 / price_token1_per_token0
    } else {
        0.0
    };

    PoolPrice {
        pool_address: pool_info.address,
        token0_price: price_token1_per_token0, // How much token1 for 1 token0
        token1_price: price_token0_per_token1, // How much token0 for 1 token1
        sqrt_price_x96: pool_info.sqrt_price_x96,
        tick: pool_info.tick,
        fee: pool_info.fee,
        liquidity: pool_info.liquidity,
    }
}

// Internal helper for tick to price conversion
fn tick_to_price_internal(tick: i32, token0_decimals: u8, token1_decimals: u8) -> f64 {
    // Ensure tick is within valid range
    const MIN_TICK: i32 = -887272;
    const MAX_TICK: i32 = 887272;

    let clamped_tick = tick.max(MIN_TICK).min(MAX_TICK);

    let base = 1.0001_f64;

    // For large negative ticks, use logarithmic calculation to avoid underflow
    let unscaled_price = if clamped_tick.abs() > 100000 {
        // Use logarithmic calculation for extreme ticks
        let ln_base = base.ln();
        (clamped_tick as f64 * ln_base).exp()
    } else {
        // Direct calculation for moderate ticks
        base.powi(clamped_tick)
    };

    // Apply decimal adjustment: price = P × 10^(decimals0 - decimals1)
    let decimal_adjustment = 10_f64.powf((token0_decimals as f64) - (token1_decimals as f64));

    let result = unscaled_price * decimal_adjustment;

    // Return a reasonable minimum if the result is too small
    if result == 0.0 || !result.is_finite() { 1e-20 } else { result }
}

/// Get token metadata
pub async fn get_token_metadata<P: Provider + Clone>(provider: P, token_address: Address) -> Result<TokenMetadata> {
    let contract = IERC20Metadata::new(token_address, provider);

    let name = contract.name().call().await?;
    let symbol = contract.symbol().call().await?;
    let decimals = contract.decimals().call().await?;

    Ok(TokenMetadata {
        name,
        symbol,
        decimals,
        address: token_address,
    })
}

/// Calculate pool TVL (requires token balances in the pool)
pub async fn calculate_pool_tvl<P: Provider + Clone>(provider: P, pool_info: &PoolInfo, token0_price_usd: Option<f64>, token1_price_usd: Option<f64>) -> Result<PoolTVL> {
    let token0_contract = IERC20Metadata::new(pool_info.token0, provider.clone());
    let token1_contract = IERC20Metadata::new(pool_info.token1, provider.clone());

    // Get pool's token balances
    let token0_balance = token0_contract.balanceOf(pool_info.address).call().await?;
    let token1_balance = token1_contract.balanceOf(pool_info.address).call().await?;

    // Convert to human-readable amounts
    let token0_amount = token0_balance.to::<u128>() as f64 / 10f64.powi(pool_info.token0_decimals as i32);
    let token1_amount = token1_balance.to::<u128>() as f64 / 10f64.powi(pool_info.token1_decimals as i32);

    // Calculate USD value if prices provided
    let total_value_usd = match (token0_price_usd, token1_price_usd) {
        (Some(price0), Some(price1)) => Some(token0_amount * price0 + token1_amount * price1),
        _ => None,
    };

    Ok(PoolTVL {
        pool_address: pool_info.address,
        token0_amount,
        token1_amount,
        total_value_usd,
    })
}

/// Get multiple pool prices in batch
pub async fn get_pools_batch<P: Provider + Clone>(provider: P, pool_addresses: Vec<Address>) -> Result<Vec<PoolInfo>> {
    let mut pools = Vec::new();

    for address in pool_addresses {
        match get_pool_info(provider.clone(), address).await {
            Ok(pool) => pools.push(pool),
            Err(e) => {
                tracing::warn!("Failed to get pool info for {}: {}", address, e);
            }
        }
    }

    Ok(pools)
}

/// Find arbitrage opportunity between two pools
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub pool1: Address,
    pub pool2: Address,
    pub price_difference_percent: f64,
    pub profitable: bool,
}

/// Compare prices between two pools for arbitrage
pub fn find_arbitrage_opportunity(pool1_price: &PoolPrice, pool2_price: &PoolPrice, min_profit_percent: f64) -> ArbitrageOpportunity {
    let price_diff_percent = ((pool1_price.token0_price - pool2_price.token0_price) / pool1_price.token0_price).abs() * 100.0;

    ArbitrageOpportunity {
        pool1: pool1_price.pool_address,
        pool2: pool2_price.pool_address,
        price_difference_percent: price_diff_percent,
        profitable: price_diff_percent > min_profit_percent,
    }
}

/// Monitor pool events (Swap, Mint, Burn)
pub async fn monitor_pool_events<T: Network>(provider: &RootProvider<T>, pool_address: Address, from_block: u64, to_block: u64) -> Result<Vec<alloy::rpc::types::Log>> {
    // Swap event signature
    const SWAP_EVENT: &str = "Swap(address,address,int256,int256,uint160,uint128,int24)";

    let filter = Filter::new().address(pool_address).event(SWAP_EVENT).from_block(from_block).to_block(to_block);

    let logs = provider.get_logs(&filter).await?;
    Ok(logs)
}

// ===== UTILITY FUNCTIONS =====

/// Convert tick to price
pub fn tick_to_price(tick: i32, token0_decimals: u8, token1_decimals: u8) -> f64 {
    tick_to_price_internal(tick, token0_decimals, token1_decimals)
}

/// Convert price to tick
pub fn price_to_tick(price: f64, token0_decimals: u8, token1_decimals: u8) -> i32 {
    let decimals_diff = token1_decimals as i32 - token0_decimals as i32;
    let adjusted_price = price / 10_f64.powi(decimals_diff);
    (adjusted_price.ln() / 1.0001_f64.ln()).round() as i32
}
