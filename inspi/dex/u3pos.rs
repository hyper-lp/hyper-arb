use alloy::{
    primitives::{Address, U128, U256},
    providers::ProviderBuilder,
};
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tracing::{debug, info, warn};

use crate::{
    constants::{BASIS_POINT_DENOMINATOR, DEFAULT_RATIO_SPLIT, PERCENT_MULTIPLIER, TICK_BASE},
    core::api::HyperLiquidAPI,
    core::precompile::PrecompileReader,
    types::config::BotConfig,
};

// ===== ALLOY CONTRACT DEFINITIONS =====

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IUniswapV3Factory,
    "src/shd/misc/abis/hyperswap/IPoolFactory.json"
);

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IUniswapV3Pool,
    "src/shd/misc/abis/hyperswap/IPool.json"
);

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    INonfungiblePositionManager,
    "src/shd/misc/abis/hyperswap/IPositions.json"
);

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IHyperSwapRouter,
    "src/shd/misc/abis/hyperswap/IRouter.json"
);

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IProjectXRouter,
    "src/shd/misc/abis/projectx/IRouter.json"
);

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IERC20,
    "src/shd/misc/abis/IERC20.json"
);

alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IWETH9,
    "src/shd/misc/abis/IWeth9.json"
);

// ===== DATA STRUCTURES =====

/// Uniswap V3 pool state and configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolInfo {
    pub address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub tick_spacing: i32,
    pub liquidity: U128,
    pub sqrt_price_x96: U256,
    pub tick: i32,
    pub observation_index: u16,
    pub observation_cardinality: u16,
    pub observation_cardinality_next: u16,
    pub fee_protocol: u8,
    pub unlocked: bool,
}

/// Liquidity position data from NFT PositionManager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpPosition {
    /// NFT token ID
    pub token_id: U256,
    /// Position nonce for permit
    pub nonce: alloy::primitives::Uint<96, 2>,
    /// Approved operator address
    pub operator: Address,
    /// First token in pair
    pub token0: Address,
    /// Second token in pair
    pub token1: Address,
    /// Pool fee tier in basis points
    pub fee: u32,
    /// Lower tick boundary
    pub tick_lower: i32,
    /// Upper tick boundary
    pub tick_upper: i32,
    /// Position liquidity
    pub liquidity: u128,
    pub fee_growth_inside0_last_x128: U256,
    pub fee_growth_inside1_last_x128: U256,
    pub tokens_owed0: u128,            // Raw u128 from contract
    pub tokens_owed1: u128,            // Raw u128 from contract
    pub pool_address: Option<Address>, // Calculated from token0, token1, fee
    // Enhanced fields with metadata and price conversion
    pub token0_symbol: Option<String>,
    pub token1_symbol: Option<String>,
    pub price_lower: Option<f64>,                // Price at tick_lower
    pub price_upper: Option<f64>,                // Price at tick_upper
    pub price_range_description: Option<String>, // Human readable price range
    // Rebalancing analysis fields
    pub rebalancing_info: Option<RebalancingInfo>,
}

/// Information about rebalancing requirements for an LP position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RebalancingInfo {
    pub current_price: f64,         // Token1/Token0 price (how much token0 needed for 1 token1)
    pub current_price_inverse: f64, // Token0/Token1 price (how much token1 needed for 1 token0)
    pub target_tick_lower: i32,
    pub target_tick_upper: i32,
    pub target_price_lower: f64,
    pub target_price_upper: f64,
    pub lower_distance_bps: i32,      // Distance from target lower to current lower in bps
    pub upper_distance_bps: i32,      // Distance from target upper to current upper in bps
    pub token0_compensation_bps: i32, // How much to adjust token0 (can be negative)
    pub token1_compensation_bps: i32, // How much to adjust token1 (can be negative)
    pub swap_direction: SwapDirection,
    pub swap_amount_token0: f64, // Amount of token0 to swap (positive means sell token0)
    pub swap_amount_token1: f64, // Amount of token1 to swap (positive means sell token1)
    pub rebalancing_description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SwapDirection {
    NoSwapNeeded,
    Token0ToToken1, // Sell token0, buy token1
    Token1ToToken0, // Sell token1, buy token0
}

/// User's complete LP portfolio data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserLpPortfolio {
    pub user: Address,
    pub total_positions: u64,
    pub positions: Vec<LpPosition>,
    pub pools: Vec<PoolInfo>,
    pub total_pools: usize,
}

/// Statistics for a specific pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolStats {
    pub pool_info: PoolInfo,
    pub token0_symbol: Option<String>,
    pub token1_symbol: Option<String>,
    pub price_token0_per_token1: Option<f64>,
    pub price_token1_per_token0: Option<f64>,
    pub tvl_usd: Option<f64>, // Total Value Locked (if price data available)
}

// ===== MAIN NFT POSITION MANAGER =====

pub struct UniswapV3NftManager {
    factory_address: Address,
    position_manager_address: Address,
    rpc_url: String,
    api_client: HyperLiquidAPI,
    _config: BotConfig,
}

impl UniswapV3NftManager {
    /// Create a new Uniswap V3 NFT Manager instance
    pub fn new(config: &BotConfig, dex_name: &str) -> Result<Self> {
        // Find the DEX configuration
        let dex_config = config
            .dex
            .iter()
            .find(|d| d.name == dex_name)
            .ok_or_else(|| eyre::eyre!("DEX '{}' not found in configuration", dex_name))?;

        // Parse addresses
        let factory_address = Address::from_str(&dex_config.factory)?;
        let position_manager_address = Address::from_str(&dex_config.position_manager)?;

        // info!(
        //      "🔧 Initialized {} V3 NFT Manager - Factory: {}, PositionManager: {}",
        //     dex_name,
        //     factory_address, position_manager_address
        // );

        // Initialize the API client for token metadata resolution
        let api_client = HyperLiquidAPI::new(config);

        Ok(Self {
            factory_address,
            position_manager_address,
            rpc_url: config.global.rpc_endpoint.clone(),
            api_client,
            _config: config.clone(),
        })
    }

    /// Get all LP positions for a specific user/EOA
    pub async fn get_user_lp_positions(&self, vault: &crate::types::config::TrackingConfig) -> Result<Vec<LpPosition>> {
        let user: Address = vault.address.parse()?;
        // info!("📊 Fetching LP positions for user: {}", user);

        // Build provider
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        // Create position manager contract instance
        let position_manager = INonfungiblePositionManager::new(self.position_manager_address, provider);

        // Get the number of NFT positions owned by the user
        let balance = position_manager.balanceOf(user).call().await?;

        if balance == U256::ZERO {
            info!("[{}] User owns 0 LP positions", vault.format_log_info());
            return Ok(Vec::new());
        }

        let balance_u64 = u64::try_from(balance).unwrap_or(0);
        info!("[{}] User {} owns {} LP positions, checking from newest to oldest...", vault.format_log_info(), user, balance_u64);

        let mut positions = Vec::new();
        let mut positions_checked = 0;
        let mut active_positions_found = 0;

        // Iterate in reverse (newest positions first) to find active positions faster
        for i in (0..balance_u64).rev() {
            positions_checked += 1;

            // Log progress every 10 positions to avoid spam
            if positions_checked % 10 == 0 {
                debug!("Checked {} positions, found {} active", positions_checked, active_positions_found);
            }
            match position_manager.tokenOfOwnerByIndex(user, U256::from(i)).call().await {
                Ok(token_id) => {
                    // debug!("📍 Processing position token ID: {}", token_id);

                    // Get detailed position information
                    match position_manager.positions(token_id).call().await {
                        Ok(pos_result) => {
                            if pos_result.liquidity > 0 {
                                let tick_lower = i32::try_from(pos_result.tickLower).unwrap_or(0);
                                let tick_upper = i32::try_from(pos_result.tickUpper).unwrap_or(0);

                                // Resolve token symbols
                                let token0_symbol = resolve_token_symbol(&self.api_client, pos_result.token0).await;
                                let token1_symbol = resolve_token_symbol(&self.api_client, pos_result.token1).await;

                                // Convert ticks to prices with decimal adjustment
                                // Use token-specific decimal defaults for accurate price calculation
                                let (decimals0, decimals1) = get_token_decimals_defaults(pos_result.token0, pos_result.token1);
                                let price_lower = tick_to_price(tick_lower, decimals0, decimals1);
                                let price_upper = tick_to_price(tick_upper, decimals0, decimals1);

                                // Create human-readable price range description
                                let price_range_description = Some(create_price_range_description(price_lower, price_upper, &token0_symbol, &token1_symbol));

                                let token0_display = token0_symbol.clone().unwrap_or_else(|| get_token_friendly_name(pos_result.token0));
                                let token1_display = token1_symbol.clone().unwrap_or_else(|| get_token_friendly_name(pos_result.token1));

                                // Try to get current pool price for range info display
                                let (range_info, _current_tick_opt) = match self.get_pool_info(pos_result.token0, pos_result.token1, u32::try_from(pos_result.fee).unwrap_or(0)).await {
                                    Ok(Some(pool_info)) => {
                                        let current_price = tick_to_price(pool_info.tick, decimals0, decimals1);
                                        let lower_bp = ((price_lower - current_price) / current_price * BASIS_POINT_DENOMINATOR).round() as i32;
                                        let upper_bp = ((price_upper - current_price) / current_price * BASIS_POINT_DENOMINATOR).round() as i32;

                                        (format!(" | Range: {:.0}bp to +{:.0}bp from spot", lower_bp, upper_bp), Some(pool_info.tick))
                                    }
                                    _ => (String::new(), None),
                                };

                                // Create position without rebalancing info (will be calculated when needed with correct vault config)
                                let position = LpPosition {
                                    token_id,
                                    nonce: pos_result.nonce,
                                    operator: pos_result.operator,
                                    token0: pos_result.token0,
                                    token1: pos_result.token1,
                                    fee: u32::try_from(pos_result.fee).unwrap_or(0),
                                    tick_lower,
                                    tick_upper,
                                    liquidity: pos_result.liquidity,
                                    fee_growth_inside0_last_x128: pos_result.feeGrowthInside0LastX128,
                                    fee_growth_inside1_last_x128: pos_result.feeGrowthInside1LastX128,
                                    tokens_owed0: pos_result.tokensOwed0,
                                    tokens_owed1: pos_result.tokensOwed1,
                                    pool_address: None, // Will be calculated later if needed
                                    // Enhanced fields
                                    token0_symbol: token0_symbol.clone(),
                                    token1_symbol: token1_symbol.clone(),
                                    price_lower: Some(price_lower),
                                    price_upper: Some(price_upper),
                                    price_range_description,
                                    rebalancing_info: None, // Not calculated here - will be calculated with proper vault config when needed
                                };

                                info!(
                                    "Added position: {}/{} fee={} | Liquidity: {} | Price range: {:.6} - {:.6}{} | Description: {}",
                                    token0_display,
                                    token1_display,
                                    position.fee,
                                    pos_result.liquidity,
                                    price_lower,
                                    price_upper,
                                    range_info,
                                    position.price_range_description.as_ref().unwrap_or(&"N/A".to_string())
                                );

                                // Note: Rebalancing calculations removed from display function
                                // They will be performed with proper vault configuration when actually needed for rebalancing

                                positions.push(position);
                                active_positions_found += 1;

                                // Early exit: once we find positions with liquidity, we can stop
                                // (newest positions are most likely to be the active ones)
                                if active_positions_found >= 1 {
                                    info!("Found {} active position(s) after checking {} positions, stopping search", active_positions_found, positions_checked);
                                    break;
                                }
                            } else {
                                // Position has zero liquidity, continue searching
                                // debug!("Position {} has zero liquidity, continuing search", token_id);
                            }
                        }
                        Err(e) => {
                            warn!("Failed to fetch position data for token {}: {}", token_id, e);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to get token ID at index {}: {}", i, e);
                }
            }
        }

        if positions.is_empty() && positions_checked > 0 {
            info!("[{}] No active LP positions found after checking {} positions", vault.format_log_info(), positions_checked);
        } else if positions.is_empty() {
            info!("No LP positions to check");
        } else {
            info!(
                "[{}] Found {} active LP position(s) after checking {} position(s)",
                vault.format_log_info(),
                positions.len(),
                positions_checked
            );
        }
        Ok(positions)
    }

    /// Get pool information for a specific token pair and fee
    pub async fn get_pool_info(&self, token0: Address, token1: Address, fee: u32) -> Result<Option<PoolInfo>> {
        // debug!("🏊 Fetching pool info for {}/{} fee={}", token0, token1, fee);

        // Build provider
        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        // Create factory contract instance
        let factory = IUniswapV3Factory::new(self.factory_address, &provider);

        // Get pool address from factory
        let pool_address = factory.getPool(token0, token1, alloy::primitives::Uint::<24, 1>::from(fee)).call().await?;

        if pool_address == Address::ZERO {
            debug!("No pool found for {}/{} fee={}", token0, token1, fee);
            return Ok(None);
        }

        // Create pool contract instance
        let pool = IUniswapV3Pool::new(pool_address, &provider);

        // Fetch pool data
        let slot0_result = pool.slot0().call().await?;
        let liquidity = pool.liquidity().call().await?;
        let tick_spacing = pool.tickSpacing().call().await?;

        let pool_info = PoolInfo {
            address: pool_address,
            token0,
            token1,
            fee,
            tick_spacing: i32::try_from(tick_spacing).unwrap_or(0),
            liquidity: U128::from(liquidity),
            sqrt_price_x96: U256::from(slot0_result.sqrtPriceX96),
            tick: i32::try_from(slot0_result.tick).unwrap_or(0),
            observation_index: slot0_result.observationIndex,
            observation_cardinality: slot0_result.observationCardinality,
            observation_cardinality_next: slot0_result.observationCardinalityNext,
            fee_protocol: slot0_result.feeProtocol,
            unlocked: slot0_result.unlocked,
        };

        debug!("Pool found: {} liquidity={}", pool_address, pool_info.liquidity);
        Ok(Some(pool_info))
    }

    /// Get comprehensive user LP portfolio (positions + pool data)
    pub async fn get_user_portfolio(&self, vault: &crate::types::config::TrackingConfig) -> Result<UserLpPortfolio> {
        let user: Address = vault.address.parse()?;
        info!("[{}] Building comprehensive LP portfolio for user: {}", vault.format_log_info(), user);

        // Get all user positions
        let positions = self.get_user_lp_positions(vault).await?;
        let total_positions = positions.len() as u64;

        // Get unique pools from positions
        let mut unique_pools = std::collections::HashSet::new();
        for pos in &positions {
            unique_pools.insert((pos.token0, pos.token1, pos.fee));
        }

        // Fetch pool information for each unique pool
        let mut pools = Vec::new();
        for (token0, token1, fee) in unique_pools {
            match self.get_pool_info(token0, token1, fee).await {
                Ok(Some(pool_info)) => {
                    pools.push(pool_info);
                }
                Ok(None) => {
                    warn!("Pool not found for {}/{} fee={}", token0, token1, fee);
                }
                Err(e) => {
                    warn!("Failed to fetch pool info for {}/{} fee={}: {}", token0, token1, fee, e);
                }
            }
        }

        let portfolio = UserLpPortfolio {
            user,
            total_positions,
            positions,
            total_pools: pools.len(),
            pools,
        };

        info!("Portfolio complete: {} positions across {} pools", portfolio.total_positions, portfolio.total_pools);

        Ok(portfolio)
    }

    /// Calculate pool price from sqrtPriceX96
    pub fn calculate_price_from_sqrt(&self, sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> (f64, f64) {
        // Convert sqrtPriceX96 to actual price
        // price = (sqrtPriceX96 / 2^96)^2 * (10^decimals0 / 10^decimals1)

        // Convert U256 to f64 safely
        let sqrt_price_f64 = if let Ok(as_u128) = u128::try_from(sqrt_price_x96) {
            as_u128 as f64
        } else {
            // For very large numbers, use string conversion as fallback
            sqrt_price_x96.to_string().parse::<f64>().unwrap_or(0.0)
        };

        let sqrt_price_normalized = sqrt_price_f64 / (2_f64.powf(96.0));
        let price_raw = sqrt_price_normalized.powi(2);

        let decimal_adjustment = 10_f64.powf((decimals0 as f64) - (decimals1 as f64));
        let price_token1_per_token0 = price_raw * decimal_adjustment;
        let price_token0_per_token1 = if price_token1_per_token0 != 0.0 { 1.0 / price_token1_per_token0 } else { 0.0 };

        (price_token0_per_token1, price_token1_per_token0)
    }

    /// Print user portfolio summary
    pub fn print_portfolio_summary(&self, portfolio: &UserLpPortfolio) {
        info!("==================== LP Portfolio Summary ====================");
        info!("👤 User: {}", portfolio.user);
        info!("📊 Total LP Positions: {}", portfolio.total_positions);
        info!("🏊 Total Unique Pools: {}", portfolio.total_pools);
        info!("");

        if !portfolio.positions.is_empty() {
            info!("📍 LP Positions:");
            for (i, pos) in portfolio.positions.iter().enumerate() {
                // Create enhanced token display with symbol and address
                let token0_display = if let Some(ref symbol) = pos.token0_symbol {
                    format!("{} ({})", symbol, pos.token0)
                } else {
                    format!("TOKEN ({})", pos.token0)
                };

                let token1_display = if let Some(ref symbol) = pos.token1_symbol {
                    format!("{} ({})", symbol, pos.token1)
                } else {
                    format!("TOKEN ({})", pos.token1)
                };

                info!("  {}. Token ID: {} | {}/{} (fee: {})", i + 1, pos.token_id, token0_display, token1_display, pos.fee);

                // Show tick range with converted price values (token1/token0 price)
                if let (Some(price_lower), Some(price_upper)) = (pos.price_lower, pos.price_upper) {
                    info!(
                        "     Range: tick {} (token1/token0 price: {:.8}) to {} (token1/token0 price: {:.8}) | Liquidity: {}",
                        pos.tick_lower, price_lower, pos.tick_upper, price_upper, pos.liquidity
                    );
                } else {
                    info!("     Range: tick {} to {} | Liquidity: {}", pos.tick_lower, pos.tick_upper, pos.liquidity);
                }

                // Show human-readable price range description
                if let Some(ref description) = pos.price_range_description {
                    info!("     Price Range: {}", description);
                }

                if pos.tokens_owed0 > 0 || pos.tokens_owed1 > 0 {
                    info!("     Fees Owed: {} token0, {} token1", pos.tokens_owed0, pos.tokens_owed1);
                }
                info!("");
            }
        }

        if !portfolio.pools.is_empty() {
            info!("🏊 Pool Information:");
            for (i, pool) in portfolio.pools.iter().enumerate() {
                info!("  {}. Pool: {} | {}/{} (fee: {})", i + 1, pool.address, pool.token0, pool.token1, pool.fee);
                info!("     Current Tick: {} | Total Liquidity: {}", pool.tick, pool.liquidity);
                info!("     Sqrt Price X96: {}", pool.sqrt_price_x96);
                info!("");
            }
        }

        info!("===============================================================");
    }

    /// Get all positions with their associated pool data
    pub async fn get_positions_with_pools(&self, vault: &crate::types::config::TrackingConfig) -> Result<Vec<(LpPosition, Option<PoolInfo>)>> {
        let positions = self.get_user_lp_positions(vault).await?;
        let mut positions_with_pools = Vec::new();

        for mut position in positions {
            let pool_info = self.get_pool_info(position.token0, position.token1, position.fee).await?;

            // Add pool address to position if found
            if let Some(ref pool) = pool_info {
                position.pool_address = Some(pool.address);
            }

            positions_with_pools.push((position, pool_info));
        }

        Ok(positions_with_pools)
    }
}

// ===== LP POSITION VALUATION =====

/// Represents the USD value and composition of an LP position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LpPositionValue {
    pub token0_amount: f64,
    pub token1_amount: f64,
    pub token0_usd_value: f64,
    pub token1_usd_value: f64,
    pub total_usd_value: f64,
    pub token0_percent: f64,
    pub token1_percent: f64,
    pub position_status: PositionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PositionStatus {
    InRange,
    BelowRange, // All token0
    AboveRange, // All token1
}

/// Calculate the amount of token0 and token1 in a concentrated liquidity position
/// Based on Uniswap V3 whitepaper formulas
/// @description: Calculate token amounts held in a Uniswap V3 LP position based on current price
/// @param liquidity: Liquidity amount in the position
/// @param current_tick: Current tick (price) of the pool
/// @param tick_lower: Lower tick bound of the position
/// @param tick_upper: Upper tick bound of the position
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @return (f64, f64, PositionStatus): Token0 amount, Token1 amount, and position status
/// =============================================================================
pub fn calculate_position_amounts(liquidity: u128, current_tick: i32, tick_lower: i32, tick_upper: i32, decimals0: u8, decimals1: u8) -> (f64, f64, PositionStatus) {
    // Convert liquidity to f64 for calculations
    let liquidity_f64 = liquidity as f64;

    // Calculate sqrt prices from ticks
    let sqrt_price_lower = tick_to_sqrt_price(tick_lower);
    let sqrt_price_upper = tick_to_sqrt_price(tick_upper);
    let sqrt_price_current = tick_to_sqrt_price(current_tick);

    let (amount0, amount1, status) = if current_tick < tick_lower {
        // Position is entirely in token0 (below range)
        let amount0 = liquidity_f64 * (1.0 / sqrt_price_lower - 1.0 / sqrt_price_upper);
        (amount0, 0.0, PositionStatus::BelowRange)
    } else if current_tick >= tick_upper {
        // Position is entirely in token1 (above range)
        let amount1 = liquidity_f64 * (sqrt_price_upper - sqrt_price_lower);
        (0.0, amount1, PositionStatus::AboveRange)
    } else {
        // Position is in range, contains both tokens
        let amount0 = liquidity_f64 * (1.0 / sqrt_price_current - 1.0 / sqrt_price_upper);
        let amount1 = liquidity_f64 * (sqrt_price_current - sqrt_price_lower);
        (amount0, amount1, PositionStatus::InRange)
    };

    // Apply decimal adjustments to get human-readable amounts
    let amount0_adjusted = amount0 / (10_f64.powi(decimals0 as i32));
    let amount1_adjusted = amount1 / (10_f64.powi(decimals1 as i32));

    (amount0_adjusted, amount1_adjusted, status)
}

/// Convert tick to sqrt price (sqrt of 1.0001^tick)
pub fn tick_to_sqrt_price(tick: i32) -> f64 {
    1.0001_f64.powi(tick).sqrt()
}

/// Get tick spacing for a given fee tier
pub fn get_tick_spacing_for_fee(fee: u32) -> i32 {
    match fee {
        100 => 1,     // 0.01% fee
        500 => 10,    // 0.05% fee
        3000 => 60,   // 0.3% fee
        10000 => 200, // 1% fee
        _ => 60,      // Default to 0.3% spacing
    }
}

/// Calculate rebalancing with wallet balances for optimal capital usage
/// @param current_tick: Current pool tick
/// @param position_tick_lower: Current position lower tick  
/// @param position_tick_upper: Current position upper tick
/// @param liquidity: Position liquidity amount
/// @param wallet_balance0: Current wallet balance of token0 (human-readable)
/// @param wallet_balance1: Current wallet balance of token1 (human-readable)
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @param token0_usd_price: USD price of token0
/// @param token1_usd_price: USD price of token1
/// @param target_range_bps: Target range in basis points (e.g., 1000 for ±500bp)
/// @param fee: Pool fee tier (e.g., 3000 for 0.3%)
/// @return RebalancingInfo with wallet-aware swap amounts
pub fn calculate_rebalancing_with_wallet(
    current_tick: i32, position_tick_lower: i32, position_tick_upper: i32, liquidity: u128, wallet_balance0: f64, wallet_balance1: f64, decimals0: u8, decimals1: u8, token0_usd_price: f64,
    token1_usd_price: f64, target_range_bps: i32, fee: u32,
) -> RebalancingInfo {
    // Calculate current amounts from LP position
    let (lp_amount0, lp_amount1, _) = calculate_position_amounts(liquidity, current_tick, position_tick_lower, position_tick_upper, decimals0, decimals1);

    // Total available tokens (LP withdrawal + wallet)
    let total_available0 = lp_amount0 + wallet_balance0;
    let total_available1 = lp_amount1 + wallet_balance1;

    // Calculate total USD value
    let total_value_usd = total_available0 * token0_usd_price + total_available1 * token1_usd_price;

    // Calculate target ticks for symmetrical range
    let tick_spacing = get_tick_spacing_for_fee(fee);
    let half_range_ticks = ((target_range_bps as f64 / 2.0) / PERCENT_MULTIPLIER * BASIS_POINT_DENOMINATOR / TICK_BASE.ln()) as i32;
    let target_tick_lower = ((current_tick - half_range_ticks) / tick_spacing) * tick_spacing;
    let target_tick_upper = ((current_tick + half_range_ticks) / tick_spacing) * tick_spacing;

    // Calculate the optimal amounts for this symmetrical range
    let (optimal_amount0, optimal_amount1) = calculate_optimal_amounts_for_range(
        total_value_usd,
        current_tick,
        target_tick_lower,
        target_tick_upper,
        token0_usd_price,
        token1_usd_price,
        decimals0,
        decimals1,
    );

    // These are the target amounts we want after swapping
    let target_amount0 = optimal_amount0;
    let target_amount1 = optimal_amount1;

    // Calculate the target ratios for display
    let target_ratio0 = if total_value_usd > 0.0 {
        (target_amount0 * token0_usd_price) / total_value_usd
    } else {
        DEFAULT_RATIO_SPLIT
    };
    let target_ratio1 = if total_value_usd > 0.0 {
        (target_amount1 * token1_usd_price) / total_value_usd
    } else {
        DEFAULT_RATIO_SPLIT
    };

    // Calculate swap amounts (positive = sell, negative = buy)
    let swap_amount_token0 = total_available0 - target_amount0;
    let swap_amount_token1 = total_available1 - target_amount1;

    // Determine swap direction
    let swap_direction = if swap_amount_token0.abs() < 0.0001 && swap_amount_token1.abs() < 0.0001 {
        SwapDirection::NoSwapNeeded
    } else if swap_amount_token0 > 0.0 {
        SwapDirection::Token0ToToken1
    } else {
        SwapDirection::Token1ToToken0
    };

    // Convert prices
    let current_price = tick_to_price(current_tick, decimals0, decimals1);
    let current_price_inverse = if current_price > 0.0 { 1.0 / current_price } else { 0.0 };
    let target_price_lower = tick_to_price(target_tick_lower, decimals0, decimals1);
    let target_price_upper = tick_to_price(target_tick_upper, decimals0, decimals1);

    // Calculate distances
    let position_price_lower = tick_to_price(position_tick_lower, decimals0, decimals1);
    let position_price_upper = tick_to_price(position_tick_upper, decimals0, decimals1);
    let lower_distance_bps = ((target_price_lower - position_price_lower) / position_price_lower * BASIS_POINT_DENOMINATOR) as i32;
    let upper_distance_bps = ((target_price_upper - position_price_upper) / position_price_upper * BASIS_POINT_DENOMINATOR) as i32;

    // Build description
    let description = format!(
        "Wallet-aware rebalancing: Total available {:.6} token0 + {:.6} token1 (${:.2}). \
         Target ratio for {}bp range (±{}bp from center): {:.1}%/{:.1}%. \
         Swap: {}",
        total_available0,
        total_available1,
        total_value_usd,
        target_range_bps,
        target_range_bps / 2,
        target_ratio0 * PERCENT_MULTIPLIER,
        target_ratio1 * PERCENT_MULTIPLIER,
        match swap_direction {
            SwapDirection::NoSwapNeeded => "None needed".to_string(),
            SwapDirection::Token0ToToken1 => format!("{:.6} token0 → token1", swap_amount_token0.abs()),
            SwapDirection::Token1ToToken0 => format!("{:.6} token1 → token0", swap_amount_token1.abs()),
        }
    );

    RebalancingInfo {
        current_price,
        current_price_inverse,
        target_tick_lower,
        target_tick_upper,
        target_price_lower,
        target_price_upper,
        lower_distance_bps,
        upper_distance_bps,
        token0_compensation_bps: 0,
        token1_compensation_bps: 0,
        swap_direction,
        swap_amount_token0,
        swap_amount_token1,
        rebalancing_description: description,
    }
}

/// Calculate optimal token amounts needed for a position at given tick range
/// This ensures no tokens are returned when minting the position
/// @param total_value_usd: Total USD value available to deposit
/// @param current_tick: Current pool tick
/// @param tick_lower: Lower tick of target range
/// @param tick_upper: Upper tick of target range
/// @param token0_usd_price: USD price of token0
/// @param token1_usd_price: USD price of token1
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @return (amount0, amount1): Optimal token amounts in human-readable units
pub fn calculate_optimal_amounts_for_range(
    total_value_usd: f64, current_tick: i32, tick_lower: i32, tick_upper: i32, token0_usd_price: f64, token1_usd_price: f64, decimals0: u8, decimals1: u8,
) -> (f64, f64) {
    // Calculate sqrt prices
    let sqrt_price_lower = tick_to_sqrt_price(tick_lower);
    let sqrt_price_upper = tick_to_sqrt_price(tick_upper);
    let sqrt_price_current = tick_to_sqrt_price(current_tick);

    // Determine the ratio based on position in range
    let (ratio0, ratio1) = if current_tick < tick_lower {
        // Price below range: 100% token0, 0% token1
        (1.0, 0.0)
    } else if current_tick >= tick_upper {
        // Price above range: 0% token0, 100% token1
        (0.0, 1.0)
    } else {
        // Price in range: calculate exact ratio needed
        // For symmetrical ranges centered at current price, the formula simplifies

        // Check if this is a symmetrical range (within 1% tolerance)
        let lower_distance = (sqrt_price_current - sqrt_price_lower).abs();
        let upper_distance = (sqrt_price_upper - sqrt_price_current).abs();
        let is_symmetrical = (lower_distance - upper_distance).abs() / lower_distance < 0.01;

        if is_symmetrical && current_tick == tick_lower + (tick_upper - tick_lower) / 2 {
            // For perfectly centered symmetrical positions, the ratio is exactly 50/50 in value
            // This is because the liquidity distribution is balanced
            debug!("Position is symmetrical and centered - using 50/50 value split");
            (DEFAULT_RATIO_SPLIT, DEFAULT_RATIO_SPLIT)
        } else {
            // For non-symmetrical or off-center positions, calculate the exact ratio
            // amount0 = L * (1/sqrt_P - 1/sqrt_Pb)
            // amount1 = L * (sqrt_P - sqrt_Pa)

            let amount0_per_l = 1.0 / sqrt_price_current - 1.0 / sqrt_price_upper;
            let amount1_per_l = sqrt_price_current - sqrt_price_lower;

            // Convert to USD values per unit of liquidity
            let value0_per_l = amount0_per_l * token0_usd_price / (10_f64.powi(decimals0 as i32));
            let value1_per_l = amount1_per_l * token1_usd_price / (10_f64.powi(decimals1 as i32));

            // Calculate the ratio of USD values
            let total_value_per_l = value0_per_l + value1_per_l;
            if total_value_per_l > 0.0 {
                (value0_per_l / total_value_per_l, value1_per_l / total_value_per_l)
            } else {
                (DEFAULT_RATIO_SPLIT, DEFAULT_RATIO_SPLIT) // Fallback to 50/50 if calculation fails
            }
        }
    };

    // Calculate optimal amounts based on the ratio
    let optimal_value0_usd = total_value_usd * ratio0;
    let optimal_value1_usd = total_value_usd * ratio1;

    // Convert USD values to token amounts
    let optimal_amount0 = if token0_usd_price > 0.0 { optimal_value0_usd / token0_usd_price } else { 0.0 };

    let optimal_amount1 = if token1_usd_price > 0.0 { optimal_value1_usd / token1_usd_price } else { 0.0 };

    debug!("📊 Optimal amounts for range [{}, {}] at tick {}: ", tick_lower, tick_upper, current_tick);
    debug!("   Ratio: {:.1}% token0, {:.1}% token1", ratio0 * PERCENT_MULTIPLIER, ratio1 * PERCENT_MULTIPLIER);
    debug!("   Amounts: {:.6} token0, {:.6} token1", optimal_amount0, optimal_amount1);

    (optimal_amount0, optimal_amount1)
}

/// Evaluate LP position value in USD
/// @description: Evaluate the USD value and composition of an LP position
/// @param position: LP position data structure
/// @param current_pool_tick: Current price tick of the pool
/// @param token0_usd_price: USD price of token0
/// @param token1_usd_price: USD price of token1
/// @return LpPositionValue: Complete valuation with token amounts and USD values
/// =============================================================================
pub async fn evaluate_lp_position_value(position: &LpPosition, current_pool_tick: i32, token0_usd_price: f64, token1_usd_price: f64) -> Result<LpPositionValue> {
    // Get token decimals for calculation
    let (decimals0, decimals1) = get_token_decimals_defaults(position.token0, position.token1);

    // Calculate current token amounts in the position
    let (amount0, amount1, status) = calculate_position_amounts(position.liquidity, current_pool_tick, position.tick_lower, position.tick_upper, decimals0, decimals1);

    // Calculate USD values
    let token0_usd_value = amount0 * token0_usd_price;
    let token1_usd_value = amount1 * token1_usd_price;
    let total_usd_value = token0_usd_value + token1_usd_value;

    // Calculate percentages
    let token0_percent = if total_usd_value > 0.0 { (token0_usd_value / total_usd_value) * PERCENT_MULTIPLIER } else { 0.0 };
    let token1_percent = if total_usd_value > 0.0 { (token1_usd_value / total_usd_value) * PERCENT_MULTIPLIER } else { 0.0 };

    Ok(LpPositionValue {
        token0_amount: amount0,
        token1_amount: amount1,
        token0_usd_value,
        token1_usd_value,
        total_usd_value,
        token0_percent,
        token1_percent,
        position_status: status,
    })
}

/// Print LP position valuation summary
pub fn print_position_valuation(position: &LpPosition, valuation: &LpPositionValue, token0_price: f64, token1_price: f64) {
    let token0_symbol = position.token0_symbol.as_deref().unwrap_or("TOKEN0");
    let token1_symbol = position.token1_symbol.as_deref().unwrap_or("TOKEN1");

    info!("💰 ==================== LP Position Valuation ====================");
    info!("📍 Token ID: {}", position.token_id);
    info!("🎯 Position Status: {:?}", valuation.position_status);
    info!("");
    info!("📊 Token Composition:");
    info!(
        "   {} ({}): {:.6} tokens @ ${:.2} = ${:.2}",
        token0_symbol, position.token0, valuation.token0_amount, token0_price, valuation.token0_usd_value
    );
    info!(
        "   {} ({}): {:.6} tokens @ ${:.2} = ${:.2}",
        token1_symbol, position.token1, valuation.token1_amount, token1_price, valuation.token1_usd_value
    );
    info!("");
    info!("💎 Total Position Value: ${:.2} USD", valuation.total_usd_value);
    info!(
        "📈 Portfolio Allocation: {:.1}% {} / {:.1}% {}",
        valuation.token0_percent, token0_symbol, valuation.token1_percent, token1_symbol
    );
    info!("");
    info!("📍 Tick Range: {} to {} (Current liquidity: {})", position.tick_lower, position.tick_upper, position.liquidity);
    info!("💰 =================================================================");
}

// ===== HELPER FUNCTIONS =====

/// Convert a tick to price of token1 in terms of token0 with decimal adjustment
/// Formula: price = 1.0001^tick × 10^(decimals0 - decimals1)
/// This gives the price of token1/token0 (how much token0 you need to buy 1 token1)
/// - Negative ticks: token1 is cheaper (< 1 token0 per token1)  
/// - Positive ticks: token1 is more expensive (> 1 token0 per token1)
/// - Each tick represents a 0.01% price change
/// - Valid tick range: -887272 to 887272
/// @description: Convert Uniswap V3 tick to token price ratio accounting for decimal differences
/// @param tick: Pool tick value (log base 1.0001 of price)
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @return f64: Price ratio (token1/token0) adjusted for decimals
/// =============================================================================
pub fn tick_to_price(tick: i32, decimals0: u8, decimals1: u8) -> f64 {
    // Ensure tick is within valid range
    const MIN_TICK: i32 = -887272;
    const MAX_TICK: i32 = 887272;

    let clamped_tick = tick.max(MIN_TICK).min(MAX_TICK);
    if tick != clamped_tick {
        warn!("Tick {} clamped to valid range [{}, {}]", tick, MIN_TICK, MAX_TICK);
    }

    let base = 1.0001_f64;

    // Calculate unscaled price: P = 1.0001^tick
    let unscaled_price = if clamped_tick < -700000 {
        // For very large negative ticks, use logarithmic approach for better precision
        let ln_base = base.ln();
        let result = (clamped_tick as f64 * ln_base).exp();

        // Return a very small but meaningful number if underflow occurs
        if result == 0.0 || !result.is_normal() { 1e-100 } else { result }
    } else {
        base.powi(clamped_tick)
    };

    // Apply decimal adjustment: price = P × 10^(decimals0 - decimals1)
    let decimal_adjustment = 10_f64.powf((decimals0 as f64) - (decimals1 as f64));

    // Debug logging can be enabled for troubleshooting if needed
    // debug!("tick_to_price: tick={}, decimals0={}, decimals1={}, unscaled={}, adjustment={}, final={}",
    //        clamped_tick, decimals0, decimals1, unscaled_price, decimal_adjustment, adjusted_price);

    unscaled_price * decimal_adjustment
}

/// Convert a tick to price without decimal adjustment (legacy function)
/// Assumes 18 decimals for both tokens (no adjustment)
/// Use tick_to_price() with explicit decimals for accurate results
pub fn tick_to_price_legacy(tick: i32) -> f64 {
    tick_to_price(tick, 18, 18) // No decimal adjustment
}

/// Convert a price back to tick
/// Formula: tick = log(price) / log(1.0001)
pub fn price_to_tick(price: f64) -> i32 {
    (price.ln() / TICK_BASE.ln()).round() as i32
}

/// Resolve token address to symbol by calling ERC20 symbol() function
pub async fn resolve_token_symbol(_api: &HyperLiquidAPI, address: Address) -> Option<String> {
    // Always try calling the ERC20 symbol() function first for real token data
    match call_erc20_symbol(address).await {
        Ok(symbol) => {
            // debug!("Got ERC20 symbol '{}' for token {}", symbol, address);
            Some(symbol)
        }
        Err(_) => {
            // If ERC20 call fails, fall back to known token mapping
            debug!("ERC20 symbol call failed for token {}, trying known mapping", address);
            get_known_token_symbol(address)
        }
    }
}

/// Call ERC20 symbol() function on a token contract
async fn call_erc20_symbol(token_address: Address) -> Result<String> {
    // Use the same RPC endpoint as the rest of the application
    let rpc_url = "https://rpc.hyperliquid.xyz/evm";
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    // Create ERC20 contract instance
    let token_contract = IERC20::new(token_address, provider);

    // Call symbol() function
    let symbol_result = token_contract.symbol().call().await?;

    Ok(symbol_result)
}

/// Get known token symbols for common HyperEVM addresses
pub fn get_known_token_symbol(address: Address) -> Option<String> {
    let addr_str = format!("{:?}", address);

    match addr_str.as_str() {
        // Main tokens on HyperEVM
        "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb" => Some("USDT0".to_string()), // Bridged USDT
        "0x02c6a2fA58cC01A18B8D9E00eA48d65E4dF26c70" => Some("HYPE".to_string()),  // HYPE token

        // Common test addresses
        "0x5555555555555555555555555555555555555555" => Some("TEST_TOKEN".to_string()),
        "0x0000000000000000000000000000000000000000" => Some("NULL_TOKEN".to_string()),

        // Add more known tokens - these might need to be updated based on actual deployments
        "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE" => Some("ETH".to_string()), // ETH placeholder

        _ => None,
    }
}

/// Get default decimals for token pairs for price calculation
/// Returns (decimals0, decimals1) for a token pair
pub fn get_token_decimals_defaults(token0: Address, token1: Address) -> (u8, u8) {
    let addr0_str = format!("{:?}", token0);
    let addr1_str = format!("{:?}", token1);

    let decimals0 = match addr0_str.to_lowercase().as_str() {
        "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb" => 6,  // USDT0 has 6 decimals
        "0x02c6a2fa58cc01a18b8d9e00ea48d65e4df26c70" => 18, // HYPE has 18 decimals
        "0x5555555555555555555555555555555555555555" => 18, // TEST_TOKEN default
        "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" => 18, // ETH has 18 decimals
        _ => 18,                                            // Default to 18 decimals for unknown tokens
    };

    let decimals1 = match addr1_str.to_lowercase().as_str() {
        "0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb" => 6,  // USDT0 has 6 decimals
        "0x02c6a2fa58cc01a18b8d9e00ea48d65e4df26c70" => 18, // HYPE has 18 decimals
        "0x5555555555555555555555555555555555555555" => 18, // TEST_TOKEN default
        "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" => 18, // ETH has 18 decimals
        _ => 18,                                            // Default to 18 decimals for unknown tokens
    };

    (decimals0, decimals1)
}

/// Create a human-readable price range description  
/// Shows the token1/token0 price range (how much token0 needed to buy 1 token1)
pub fn create_price_range_description(price_lower: f64, price_upper: f64, token0_symbol: &Option<String>, token1_symbol: &Option<String>) -> String {
    let token0_name = token0_symbol.as_deref().unwrap_or("TOKEN0");
    let token1_name = token1_symbol.as_deref().unwrap_or("TOKEN1");

    // Format: "price_lower - price_upper TOKEN1/TOKEN0"
    // This represents the range of how much TOKEN0 you need to buy 1 TOKEN1
    format!("{:.6} - {:.6} {}/{}", price_lower, price_upper, token1_name, token0_name)
}

/// Check if an address is a known test/placeholder token
pub fn is_test_token(address: Address) -> bool {
    let addr_str = format!("{:?}", address);
    addr_str == "0x5555555555555555555555555555555555555555" || addr_str.contains("555555") || addr_str == "0x0000000000000000000000000000000000000000"
}

/// Get a friendly name for known tokens (including test tokens)
pub fn get_token_friendly_name(address: Address) -> String {
    let addr_str = format!("{:?}", address);

    if addr_str == "0x5555555555555555555555555555555555555555" {
        return format!("TEST_TOKEN ({})", &addr_str[..10]);
    }

    if addr_str == "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb" {
        return format!("USDT0 ({})", &addr_str[..10]);
    }

    if addr_str == "0x02c6a2fA58cC01A18B8D9E00eA48d65E4dF26c70" {
        return format!("HYPE ({})", &addr_str[..10]);
    }

    // For unknown tokens, just show first 10 chars
    format!("TOKEN ({})", &addr_str[..10])
}

/// Dynamically assign USD prices based on token addresses and pool price
/// ! Huge warning there
/// @description: Determines which token is HYPE and which is USDT, then assigns correct USD prices
/// @param token0: Token0 contract address
/// @param token1: Token1 contract address  
/// @param pool_price: Current pool price (token1/token0 ratio)
/// @param config: Bot configuration with token addresses
/// @return (f64, f64): USD prices for token0 and token1 respectively
/// =============================================================================
pub fn get_token_usd_prices_by_address(token0: Address, token1: Address, pool_price: f64, config: &BotConfig) -> (f64, f64) {
    let token0_str = format!("{:?}", token0).to_lowercase();
    let token1_str = format!("{:?}", token1).to_lowercase();

    // Get token addresses from config
    let hype_address = config.hyperevm.wrapped_hype_token_address.to_lowercase();
    let usdt_address = config.hyperevm.usdt_token_address.to_lowercase();

    // Check if this is a HYPE/USDT pool
    let is_token0_hype = token0_str == hype_address;
    let is_token1_hype = token1_str == hype_address;
    let is_token0_usdt = token0_str == usdt_address;
    let is_token1_usdt = token1_str == usdt_address;

    if (is_token0_hype && is_token1_usdt) || (is_token0_usdt && is_token1_hype) {
        // This is a HYPE/USDT pool
        if is_token0_hype && is_token1_usdt {
            // Token0=HYPE, Token1=USDT
            // pool_price = USDT per HYPE, so HYPE = pool_price USD
            return (pool_price, 1.0);
        } else {
            // Token0=USDT, Token1=HYPE
            // pool_price = HYPE per USDT, so HYPE = 1/pool_price USD
            return (1.0, 1.0 / pool_price);
        }
    }

    // Fallback for unknown token pairs - assume token1 is the quote currency
    // This maintains backward compatibility with existing logic
    warn!("Unknown token pair: {} / {} - using fallback pricing", token0_str, token1_str);
    (pool_price, 1.0)
}

/// Calculate rebalancing information for an LP position with real USD prices
/// @description: Calculate rebalancing requirements using real USD prices from Hyperliquid oracles
/// @param current_tick: Current pool price tick
/// @param position_tick_lower: Current position lower tick
/// @param position_tick_upper: Current position upper tick
/// @param liquidity: Position liquidity amount
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @param token0_address: Token0 contract address for price lookup
/// @param token1_address: Token1 contract address for price lookup
/// @param config: Bot configuration with RPC settings
/// @return RebalancingInfo: Analysis with accurate USD-based rebalancing calculations
/// =============================================================================
pub async fn calculate_rebalancing_info_with_oracle(
    current_tick: i32, position_tick_lower: i32, position_tick_upper: i32, liquidity: u128, decimals0: u8, decimals1: u8, token0_address: Address, token1_address: Address, target_range_bps: u16,
    config: &BotConfig,
) -> Result<RebalancingInfo> {
    let precompile_reader = PrecompileReader::new(config);

    // Get real USD prices from oracle
    let (token0_usd_price, token1_usd_price) = get_token_usd_prices(&precompile_reader, token0_address, token1_address).await?;

    debug!("💰 Real USD prices: token0=${:.2}, token1=${:.2}", token0_usd_price, token1_usd_price);

    // Calculate the rest using the original logic but with real USD prices
    Ok(calculate_rebalancing_info_internal(
        current_tick,
        position_tick_lower,
        position_tick_upper,
        liquidity,
        decimals0,
        decimals1,
        token0_usd_price,
        token1_usd_price,
        target_range_bps.into(),
    ))
}

/// Calculate rebalancing information for an LP position with token address detection
/// @description: Calculate rebalancing requirements for LP position to achieve target range and 50/50 USD split
/// @param current_tick: Current pool price tick
/// @param position_tick_lower: Current position lower tick
/// @param position_tick_upper: Current position upper tick
/// @param liquidity: Position liquidity amount
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @param token0: Token0 contract address
/// @param token1: Token1 contract address
/// @return RebalancingInfo: Complete analysis with target ticks, distances, and swap requirements
/// =============================================================================
pub fn calculate_rebalancing_info_with_addresses(
    current_tick: i32, position_tick_lower: i32, position_tick_upper: i32, liquidity: u128, decimals0: u8, decimals1: u8, token0: Address, token1: Address, target_range_bps: i32, config: &BotConfig,
) -> RebalancingInfo {
    let current_price = tick_to_price(current_tick, decimals0, decimals1);

    // Dynamically assign USD prices based on token addresses
    let (token0_usd_price, token1_usd_price) = get_token_usd_prices_by_address(token0, token1, current_price, config);

    calculate_rebalancing_info_internal(
        current_tick,
        position_tick_lower,
        position_tick_upper,
        liquidity,
        decimals0,
        decimals1,
        token0_usd_price,
        token1_usd_price,
        target_range_bps,
    )
}

/// Calculate rebalancing information for an LP position (legacy version with assumptions)
/// @description: Calculate rebalancing requirements for LP position to achieve target range and 50/50 USD split
/// @param current_tick: Current pool price tick
/// @param position_tick_lower: Current position lower tick
/// @param position_tick_upper: Current position upper tick
/// @param liquidity: Position liquidity amount
/// @param decimals0: Token0 decimal places
/// @param decimals1: Token1 decimal places
/// @return RebalancingInfo: Complete analysis with target ticks, distances, and swap requirements
/// =============================================================================
pub fn calculate_rebalancing_info(current_tick: i32, position_tick_lower: i32, position_tick_upper: i32, liquidity: u128, decimals0: u8, decimals1: u8, target_range_bps: u16) -> RebalancingInfo {
    let current_price = tick_to_price(current_tick, decimals0, decimals1);
    // Legacy fallback: assume token0=HYPE, token1=USDT based on current price semantics
    let token0_usd_price = current_price; // HYPE price in USD 
    let token1_usd_price = 1.0; // USDT = $1

    calculate_rebalancing_info_internal(
        current_tick,
        position_tick_lower,
        position_tick_upper,
        liquidity,
        decimals0,
        decimals1,
        token0_usd_price,
        token1_usd_price,
        target_range_bps.into(),
    )
}

/// Internal rebalancing calculation with provided USD prices
fn calculate_rebalancing_info_internal(
    current_tick: i32, position_tick_lower: i32, position_tick_upper: i32, liquidity: u128, decimals0: u8, decimals1: u8, token0_usd_price: f64, token1_usd_price: f64, target_range_bps: i32,
) -> RebalancingInfo {
    // Calculate current price from tick (token1/token0 - how much token0 needed to buy 1 token1)
    let current_price = tick_to_price(current_tick, decimals0, decimals1);

    // Also calculate inverse price (token0/token1 - how much token1 needed to buy 1 token0)
    let current_price_inverse = if current_price > 0.0 { 1.0 / current_price } else { 0.0 };

    // Log both price representations for clarity
    info!(
        "Current prices - Token1/Token0: {:.6} | Token0/Token1: {:.6} | Tick: {}",
        current_price, current_price_inverse, current_tick
    );

    // Calculate target range using configured range in bps
    let range_factor = target_range_bps as f64 / BASIS_POINT_DENOMINATOR; // Convert bps to percentage (e.g., 500 bps = 0.05 = 5%)
    let target_price_lower = current_price * (1.0 - range_factor);
    let target_price_upper = current_price * (1.0 + range_factor);

    // Convert target prices back to ticks
    let target_tick_lower = price_to_tick_with_decimals(target_price_lower, decimals0, decimals1);
    let target_tick_upper = price_to_tick_with_decimals(target_price_upper, decimals0, decimals1);

    // Calculate current position prices
    let position_price_lower = tick_to_price(position_tick_lower, decimals0, decimals1);
    let position_price_upper = tick_to_price(position_tick_upper, decimals0, decimals1);

    // Calculate distance in basis points from target range to current range
    let lower_distance_bps = ((position_price_lower - target_price_lower) / current_price * BASIS_POINT_DENOMINATOR).round() as i32;
    let upper_distance_bps = ((position_price_upper - target_price_upper) / current_price * BASIS_POINT_DENOMINATOR).round() as i32;

    // Calculate average compensation needed (how far off the position is from target)
    let avg_distance_bps = (lower_distance_bps + upper_distance_bps) / 2;

    // Determine compensation in terms of token ratios
    let token0_compensation_bps: i32;
    let token1_compensation_bps: i32;

    if avg_distance_bps > 0 {
        // Position is wider than target, need to narrow
        token0_compensation_bps = -avg_distance_bps / 2;
        token1_compensation_bps = -avg_distance_bps / 2;
    } else {
        // Position is narrower than target, need to widen
        token0_compensation_bps = (-avg_distance_bps) / 2;
        token1_compensation_bps = (-avg_distance_bps) / 2;
    }

    // Calculate current token amounts in the position
    let (current_amount0, current_amount1, _) = calculate_position_amounts(liquidity, current_tick, position_tick_lower, position_tick_upper, decimals0, decimals1);

    // Calculate USD values
    let current_value0_usd = current_amount0 * token0_usd_price;
    let current_value1_usd = current_amount1 * token1_usd_price;
    let total_value_usd = current_value0_usd + current_value1_usd;

    // Calculate optimal amounts for the NEW tick range
    // Since the target range is symmetrical around current price, and we're at the center,
    // the optimal ratio should be close to 50/50 in USD terms (but exact amounts depend on liquidity math)
    let (target_amount0, target_amount1) = calculate_optimal_amounts_for_range(
        total_value_usd,
        current_tick,      // We're rebalancing AT current tick
        target_tick_lower, // Symmetrical lower bound
        target_tick_upper, // Symmetrical upper bound
        token0_usd_price,
        token1_usd_price,
        decimals0,
        decimals1,
    );

    // Calculate swap amounts needed to achieve optimal ratio
    // NOTE: These are based on LP position only, wallet balance will be added in rebalance.rs
    let swap_amount_token0 = current_amount0 - target_amount0;
    let swap_amount_token1 = current_amount1 - target_amount1;

    // Debug logging for swap calculations
    debug!("💱 Optimal Ratio Rebalancing Calculation:");
    debug!(
        "Current from LP: {:.6} token0 (${:.2}) + {:.6} token1 (${:.2}) = ${:.2} total",
        current_amount0, current_value0_usd, current_amount1, current_value1_usd, total_value_usd
    );
    debug!(
        "Target for range [{}, {}]: {:.6} token0 (${:.2}) + {:.6} token1 (${:.2})",
        target_tick_lower,
        target_tick_upper,
        target_amount0,
        target_amount0 * token0_usd_price,
        target_amount1,
        target_amount1 * token1_usd_price
    );
    debug!("Swap needed (before wallet adjustment): {:.6} token0, {:.6} token1", swap_amount_token0, swap_amount_token1);

    // Determine swap direction
    let swap_direction = if swap_amount_token0.abs() < 0.0001 && swap_amount_token1.abs() < 0.0001 {
        SwapDirection::NoSwapNeeded
    } else if swap_amount_token0 > 0.0 {
        SwapDirection::Token0ToToken1 // Need to sell token0, buy token1
    } else {
        SwapDirection::Token1ToToken0 // Need to sell token1, buy token0
    };

    // Create enhanced description with price information and optimal ratio rebalancing logic
    let optimal_ratio0 = if total_value_usd > 0.0 {
        target_amount0 * token0_usd_price / total_value_usd * PERCENT_MULTIPLIER
    } else {
        0.0
    };
    let optimal_ratio1 = if total_value_usd > 0.0 {
        target_amount1 * token1_usd_price / total_value_usd * PERCENT_MULTIPLIER
    } else {
        0.0
    };

    let rebalancing_description = format!(
        "Current range: {:.0}bp to {:.0}bp from spot (Token1/Token0: {:.6}, Token0/Token1: {:.6}). Target range: {}bp total (±{}bp from center). Distance: lower {}bp, upper {}bp. Current value split: ${:.2} token0 + ${:.2} token1 = ${:.2} total. Optimal ratio for new range: {:.1}%/{:.1}%. {}",
        ((position_price_lower - current_price) / current_price * BASIS_POINT_DENOMINATOR).round(),
        ((position_price_upper - current_price) / current_price * BASIS_POINT_DENOMINATOR).round(),
        current_price,
        current_price_inverse,
        target_range_bps,
        target_range_bps / 2,
        lower_distance_bps,
        upper_distance_bps,
        current_value0_usd,
        current_value1_usd,
        total_value_usd,
        optimal_ratio0,
        optimal_ratio1,
        match swap_direction {
            SwapDirection::NoSwapNeeded => "Already optimally balanced".to_string(),
            SwapDirection::Token0ToToken1 => format!(
                "For optimal ratio: swap {:.6} token0 to token1 (${:.2})",
                swap_amount_token0.abs(),
                swap_amount_token0.abs() * token0_usd_price
            ),
            SwapDirection::Token1ToToken0 => format!(
                "For optimal ratio: swap {:.6} token1 to token0 (${:.2})",
                swap_amount_token1.abs(),
                swap_amount_token1.abs() * token1_usd_price
            ),
        }
    );

    RebalancingInfo {
        current_price,
        current_price_inverse,
        target_tick_lower,
        target_tick_upper,
        target_price_lower,
        target_price_upper,
        lower_distance_bps,
        upper_distance_bps,
        token0_compensation_bps,
        token1_compensation_bps,
        swap_direction,
        swap_amount_token0,
        swap_amount_token1,
        rebalancing_description,
    }
}

/// Get USD prices for two tokens using oracle data
pub async fn get_token_usd_prices(precompile_reader: &PrecompileReader, token0_address: Address, token1_address: Address) -> Result<(f64, f64)> {
    let token0_str = format!("{:?}", token0_address);
    let token1_str = format!("{:?}", token1_address);

    debug!("🔍 Looking up USD prices for token0={} token1={}", token0_str, token1_str);

    // Try to get prices based on known token addresses
    let token0_price = get_usd_price_for_address(precompile_reader, token0_address).await?;
    let token1_price = get_usd_price_for_address(precompile_reader, token1_address).await?;

    Ok((token0_price, token1_price))
}

/// Get USD price for a specific token address
async fn get_usd_price_for_address(precompile_reader: &PrecompileReader, address: Address) -> Result<f64> {
    let addr_str = format!("{:?}", address);

    // Known token mapping (based on your config)
    match addr_str.as_str() {
        "0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb" => {
            // USDT0 (bridged USDT) - should be close to $1
            debug!("💵 USDT0 detected, using $1.00");
            Ok(1.0)
        }
        "0x02c6a2fA58cC01A18B8D9E00eA48d65E4dF26c70" => {
            // HYPE - get from oracle
            debug!("🚀 HYPE detected, fetching from oracle");
            match precompile_reader.get_hype_price().await {
                Ok(price) => Ok(price.mark_price),
                Err(_) => {
                    warn!("Failed to get HYPE price from oracle, using fallback");
                    Ok(43.0) // Fallback price around current HYPE price
                }
            }
        }
        _ => {
            // Unknown token - try common indices or fallback to $1
            warn!("Unknown token {}, using fallback price $1.00", addr_str);
            Ok(1.0)
        }
    }
}

/// Convert price to tick with decimal adjustment
pub fn price_to_tick_with_decimals(price: f64, decimals0: u8, decimals1: u8) -> i32 {
    // Reverse the decimal adjustment first
    let decimal_adjustment = 10_f64.powf((decimals0 as f64) - (decimals1 as f64));
    let unscaled_price = price / decimal_adjustment;

    // Then calculate tick
    (unscaled_price.ln() / TICK_BASE.ln()).round() as i32
}

/// Check if an LP position is in range (active)
pub fn is_position_in_range(position: &LpPosition, current_tick: i32) -> bool {
    current_tick >= position.tick_lower && current_tick < position.tick_upper
}

/// Calculate position value ratio based on current tick
pub fn calculate_position_ratio(position: &LpPosition, current_tick: i32) -> (f64, f64) {
    // Simplified calculation - in practice you'd want more sophisticated math
    // This gives rough ratio of token0 vs token1 in the position

    if current_tick <= position.tick_lower {
        // All token0
        (1.0, 0.0)
    } else if current_tick >= position.tick_upper {
        // All token1
        (0.0, 1.0)
    } else {
        // Mixed - simplified 50/50 for now
        // In practice, calculate based on exact tick math
        (0.5, 0.5)
    }
}

// ===== EXAMPLE USAGE =====

/// Example function showing how to use the NFT Position Manager
pub async fn example_usage(config: &BotConfig, user_address: &str) -> Result<()> {
    // Initialize the manager for HyperSwap
    let nft_manager = UniswapV3NftManager::new(config, "hyperswap")?;

    // Parse user address
    let _user = Address::from_str(user_address)?;

    // Create a dummy vault config for testing
    let test_vault = crate::types::config::TrackingConfig {
        vault_name: "test_vault".to_string(),
        address: user_address.to_string(),
        monitor_dex: "hyperswap".to_string(),
        lp_target_range_bps: 500,
        rebalance_trigger_deviation_bps: 250,
        auto_create_initial_position: false,
        use_max_token_approval: true,
        use_dex_aggregator_for_swaps: false,
        skip_nft_burn: true,
    };

    // Get comprehensive portfolio
    let portfolio = nft_manager.get_user_portfolio(&test_vault).await?;

    // Print summary
    nft_manager.print_portfolio_summary(&portfolio);

    // Get positions with pool data
    let positions_with_pools = nft_manager.get_positions_with_pools(&test_vault).await?;

    for (position, pool_info) in positions_with_pools {
        if let Some(pool) = pool_info {
            let in_range = is_position_in_range(&position, pool.tick);
            let (ratio0, ratio1) = calculate_position_ratio(&position, pool.tick);

            info!(
                "Position {} is {} (ratio: {:.1}% token0, {:.1}% token1)",
                position.token_id,
                if in_range { "IN RANGE" } else { "OUT OF RANGE" },
                ratio0 * PERCENT_MULTIPLIER,
                ratio1 * PERCENT_MULTIPLIER
            );
        }
    }

    Ok(())
}

// ===== TESTS =====

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_position_in_range() {
        let position = LpPosition {
            token_id: U256::from(1),
            nonce: alloy::primitives::Uint::<96, 2>::ZERO,
            operator: Address::ZERO,
            token0: Address::ZERO,
            token1: Address::ZERO,
            fee: 3000,
            tick_lower: -1000,
            tick_upper: 1000,
            liquidity: 1000000u128,
            fee_growth_inside0_last_x128: U256::ZERO,
            fee_growth_inside1_last_x128: U256::ZERO,
            tokens_owed0: 0u128,
            tokens_owed1: 0u128,
            pool_address: None,
            // Enhanced fields
            token0_symbol: None,
            token1_symbol: None,
            price_lower: None,
            price_upper: None,
            price_range_description: None,
            rebalancing_info: None,
        };

        assert!(is_position_in_range(&position, -500));
        assert!(is_position_in_range(&position, 0));
        assert!(is_position_in_range(&position, 500));
        assert!(!is_position_in_range(&position, -1500));
        assert!(!is_position_in_range(&position, 1500));
    }

    #[test]
    fn test_position_ratio() {
        let position = LpPosition {
            token_id: U256::from(1),
            nonce: alloy::primitives::Uint::<96, 2>::ZERO,
            operator: Address::ZERO,
            token0: Address::ZERO,
            token1: Address::ZERO,
            fee: 3000,
            tick_lower: -1000,
            tick_upper: 1000,
            liquidity: 1000000u128,
            fee_growth_inside0_last_x128: U256::ZERO,
            fee_growth_inside1_last_x128: U256::ZERO,
            tokens_owed0: 0u128,
            tokens_owed1: 0u128,
            pool_address: None,
            // Enhanced fields
            token0_symbol: None,
            token1_symbol: None,
            price_lower: None,
            price_upper: None,
            price_range_description: None,
            rebalancing_info: None,
        };

        // Test below range (all token0)
        let (ratio0, ratio1) = calculate_position_ratio(&position, -1500);
        assert_eq!(ratio0, 1.0);
        assert_eq!(ratio1, 0.0);

        // Test above range (all token1)
        let (ratio0, ratio1) = calculate_position_ratio(&position, 1500);
        assert_eq!(ratio0, 0.0);
        assert_eq!(ratio1, 1.0);

        // Test in range (mixed)
        let (ratio0, ratio1) = calculate_position_ratio(&position, 0);
        assert_eq!(ratio0, DEFAULT_RATIO_SPLIT);
        assert_eq!(ratio1, DEFAULT_RATIO_SPLIT);
    }
}
