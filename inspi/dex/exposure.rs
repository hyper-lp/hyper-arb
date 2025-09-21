use alloy::primitives::Address;
use eyre::Result;
use serde::{Deserialize, Serialize};

/// LP Position Delta/Exposure Analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpPositionDelta {
    // Pool info
    pub pool_address: Address,
    pub current_tick: i32,
    pub current_price: f64, // token1/token0 price

    // LP info
    pub token_id: u128,
    pub tick_lower: i32,
    pub tick_upper: i32,
    pub liquidity: u128,

    // Token values
    pub token0_symbol: String,
    pub token1_symbol: String,
    pub token0_amount: f64,
    pub token1_amount: f64,

    // USD values
    pub token0_price_usd: f64,
    pub token1_price_usd: f64,
    pub token0_value_usd: f64,
    pub token1_value_usd: f64,
    pub total_value_usd: f64,

    // Exposure and delta calculations
    pub token0_exposure_usd: f64,  // How much token0 exposure (price risk)
    pub token1_exposure_usd: f64,  // How much token1 exposure (price risk)
    pub concentration_factor: f64, // How concentrated vs full range
    pub delta: f64,                // Overall position delta (-1 to 1)
    pub hedge_amount_usd: f64,     // USD amount to hedge on perp
}

impl LpPositionDelta {
    /// @description: Calculate LP position delta analysis including token exposure and hedging requirements
    /// @param pool_address: Uniswap V3 pool contract address
    /// @param current_tick: Current price tick of the pool
    /// @param token_id: NFT token ID of the LP position
    /// @param tick_lower: Lower tick bound of position
    /// @param tick_upper: Upper tick bound of position
    /// @param liquidity: Liquidity amount in position
    /// @param token0_symbol: Token0 symbol for display
    /// @param token1_symbol: Token1 symbol for display
    /// @param token0_price_usd: Token0 USD price
    /// @param token1_price_usd: Token1 USD price
    /// @param decimals0: Token0 decimal places
    /// @param decimals1: Token1 decimal places
    /// @return Result<LpPositionDelta>: Complete delta analysis with exposure and hedging data
    /// =============================================================================
    pub fn calculate(
        pool_address: Address, current_tick: i32, token_id: u128, tick_lower: i32, tick_upper: i32, liquidity: u128, token0_symbol: String, token1_symbol: String, token0_price_usd: f64,
        token1_price_usd: f64, decimals0: u8, decimals1: u8,
    ) -> Result<Self> {
        // Calculate current price
        let current_price = tick_to_price(current_tick, decimals0, decimals1);

        // Calculate token amounts
        let (token0_amount, token1_amount) = calculate_amounts(liquidity, current_tick, tick_lower, tick_upper, decimals0, decimals1);

        // Calculate USD values
        let token0_value_usd = token0_amount * token0_price_usd;
        let token1_value_usd = token1_amount * token1_price_usd;
        let total_value_usd = token0_value_usd + token1_value_usd;

        // Calculate exposure (how much we're exposed to each token's price movements)
        // For LP positions, we have exposure to both tokens
        let token0_exposure_usd = token0_value_usd;
        let token1_exposure_usd = token1_value_usd;

        // Calculate concentration factor (how narrow vs wide the range is)
        let tick_range = (tick_upper - tick_lower) as f64;
        let full_range = 200000.0; // Rough estimate for "full range"
        let concentration_factor = full_range / tick_range;

        // Calculate position delta
        // Delta represents price sensitivity. For LP positions:
        // - Pure token0 position = -1 delta (loses value when token1/token0 price goes up)
        // - Pure token1 position = +1 delta (gains value when token1/token0 price goes up)
        // - Balanced position = 0 delta (neutral to price changes)
        let total_value = token0_value_usd + token1_value_usd;
        let delta = if total_value > 0.0 { (token1_value_usd - token0_value_usd) / total_value } else { 0.0 };

        // Calculate hedging amount
        // To be delta neutral, we need to hedge the token0 exposure
        let hedge_amount_usd = token0_exposure_usd;

        Ok(LpPositionDelta {
            pool_address,
            current_price,
            current_tick,
            token_id,
            tick_lower,
            tick_upper,
            liquidity,
            token0_symbol,
            token1_symbol,
            token0_amount,
            token1_amount,
            token0_price_usd,
            token1_price_usd,
            token0_value_usd,
            token1_value_usd,
            total_value_usd,
            token0_exposure_usd,
            token1_exposure_usd,
            concentration_factor,
            delta,
            hedge_amount_usd,
        })
    }

    /// @description: Print comprehensive summary of LP position delta analysis and hedging requirements
    /// @return void: Logs position details, token amounts, exposures, and hedging recommendations  
    /// =============================================================================
    pub fn print_summary(&self) {
        tracing::info!("PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP");
        tracing::info!("LP Position Delta Analysis - Token ID: {}", self.token_id);
        tracing::info!("PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP");
        tracing::info!("");
        tracing::info!("=🔍 Position Details:");
        tracing::info!("  Tick Range: {} to {}", self.tick_lower, self.tick_upper);
        tracing::info!("  Current Tick: {}", self.current_tick);
        tracing::info!("  Liquidity: {}", self.liquidity);
        tracing::info!("");
        tracing::info!("=💰 Token Amounts:");
        tracing::info!("  {}: {:.6} @ ${:.2} = ${:.2}", self.token0_symbol, self.token0_amount, self.token0_price_usd, self.token0_value_usd);
        tracing::info!("  {}: {:.6} @ ${:.2} = ${:.2}", self.token1_symbol, self.token1_amount, self.token1_price_usd, self.token1_value_usd);
        tracing::info!("  Total Value: ${:.2}", self.total_value_usd);
        tracing::info!("");
        tracing::info!("🎯 Delta/Exposure:");
        tracing::info!("  {} Exposure: ${:.2} USD", self.token0_symbol, self.token0_exposure_usd);
        tracing::info!("  {} Exposure: ${:.2} USD", self.token1_symbol, self.token1_exposure_usd);
        tracing::info!("  Concentration Factor: {:.2}x", self.concentration_factor);
        tracing::info!("");
        tracing::info!("🛡️ Delta Hedging:");
        tracing::info!("  Position Delta: {:.2}%", self.delta * 100.0);
        tracing::info!("  To be Delta Neutral:");
        tracing::info!("    -> Short ${:.2} worth of {} on perp", self.hedge_amount_usd, self.token0_symbol);
        tracing::info!("PPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPPP");
    }
}

/// Convert tick to price
fn tick_to_price(tick: i32, decimals0: u8, decimals1: u8) -> f64 {
    let base = 1.0001_f64;
    let unscaled_price = base.powi(tick);
    let decimal_adjustment = 10_f64.powf((decimals0 as f64) - (decimals1 as f64));
    unscaled_price * decimal_adjustment
}

/// Calculate token amounts for a position
fn calculate_amounts(liquidity: u128, current_tick: i32, tick_lower: i32, tick_upper: i32, decimals0: u8, decimals1: u8) -> (f64, f64) {
    let liquidity_f64 = liquidity as f64;

    // Calculate sqrt prices
    let sqrt_price_lower = tick_to_sqrt_price(tick_lower);
    let sqrt_price_upper = tick_to_sqrt_price(tick_upper);
    let sqrt_price_current = tick_to_sqrt_price(current_tick);

    let (amount0, amount1) = if current_tick < tick_lower {
        // Below range - all in token0
        let amount0 = liquidity_f64 * (1.0 / sqrt_price_lower - 1.0 / sqrt_price_upper);
        (amount0, 0.0)
    } else if current_tick >= tick_upper {
        // Above range - all in token1
        let amount1 = liquidity_f64 * (sqrt_price_upper - sqrt_price_lower);
        (0.0, amount1)
    } else {
        // In range
        let amount0 = liquidity_f64 * (1.0 / sqrt_price_current - 1.0 / sqrt_price_upper);
        let amount1 = liquidity_f64 * (sqrt_price_current - sqrt_price_lower);
        (amount0, amount1)
    };

    // Adjust for decimals
    let amount0_adjusted = amount0 / 10_f64.powi(decimals0 as i32);
    let amount1_adjusted = amount1 / 10_f64.powi(decimals1 as i32);

    (amount0_adjusted, amount1_adjusted)
}

/// Convert tick to sqrt price
fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001_f64.powi(tick).sqrt()
}
