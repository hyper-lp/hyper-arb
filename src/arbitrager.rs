use alloy::{
    network::{Ethereum, Network},
    primitives::Address,
    providers::{Provider, RootProvider},
};
use eyre::Result;
use shd::{
    dex::pool_data::{calculate_pool_prices, get_pool_info},
    types::{BotConfig, EnvConfig, PriceReference, load_bot_config_with_env},
    utils::{evm::init_allowance, misc::log_gas_prices},
};
use std::str::FromStr;
use tokio::{task, time};
use tracing::Level;
use tracing_subscriber::{EnvFilter, fmt};

// Constants
const BASIS_POINT_DENO: f64 = 10000.0; // Basis points denominator (1% = 100 bps)
const INVENTORY_CHECK_INTERVAL_BLOCKS: u64 = 10; // Check inventory every N blocks

// Inventory status for double leg mode
#[derive(Debug)]
struct InventoryStatus {
    base_token: String,
    quote_token: String,
    base_balance: f64,
    quote_balance: f64,
    base_percentage: f64,
    quote_percentage: f64,
    total_value_usd: f64,
    is_balanced: bool,
}

// Fetch price based on configured oracle reference
async fn fetch_price_by_reference(reference: &PriceReference, symbol: &str, config: &BotConfig) -> Result<f64> {
    match reference {
        PriceReference::Pyth => match symbol.to_uppercase().as_str() {
            "BTC" => shd::oracles::pyth::fetch_btc_usd_price().await,
            "ETH" => shd::oracles::pyth::fetch_eth_usd_price().await,
            "HYPE" | "WHYPE" => shd::oracles::fetch_hype_usd_price().await,
            _ => Err(eyre::eyre!("Pyth oracle doesn't support {} price", symbol)),
        },
        PriceReference::Redstone => match symbol.to_uppercase().as_str() {
            "BTC" => shd::oracles::fetch_btc_usd_price().await,
            "ETH" => shd::oracles::fetch_eth_usd_price().await,
            "HYPE" | "WHYPE" => shd::oracles::redstone::fetch_hype_usd_price().await,
            _ => {
                let redstone = shd::oracles::Redstone::new();
                redstone.get_price(symbol).await
            }
        },
        PriceReference::Hypercore => {
            let hypercore = shd::oracles::Hypercore::new(config);
            hypercore.get_price(symbol).await
        }
    }
}

// Helper function to fetch and log current balances
async fn log_current_balances<T: Network>(_provider: RootProvider<T>, target: &shd::types::ArbTarget, env: &EnvConfig, config: &BotConfig, prefix: &str) -> Result<()>
where
    RootProvider<T>: Provider + Clone,
{
    // Get wallet for this target
    let wallet = match env.get_signer_for_address(&target.address) {
        Some(s) => s,
        None => {
            return Err(eyre::eyre!("No wallet found for target {}", target.vault_name));
        }
    };
    let wallet_address = wallet.address();

    // Fetch token balances
    let (base_decimals, quote_decimals, base_balance_raw, quote_balance_raw) =
        shd::utils::evm::get_token_info_and_balances(&config.global.rpc_endpoint, &format!("{:?}", wallet_address), &target.base_token_address, &target.quote_token_address)
            .await
            .map_err(|e| eyre::eyre!(e))?;

    let base_balance = base_balance_raw as f64 / 10f64.powi(base_decimals as i32);
    let quote_balance = quote_balance_raw as f64 / 10f64.powi(quote_decimals as i32);

    // Fetch current prices
    let base_price = fetch_price_by_reference(&target.reference, &target.base_token, config).await?;
    let quote_price = if target.quote_token.to_uppercase() == "USDT0" || target.quote_token.to_uppercase() == "USDC0" {
        1.0
    } else {
        fetch_price_by_reference(&target.reference, &target.quote_token, config).await?
    };

    // Calculate USD values
    let base_value_usd = base_balance * base_price;
    let quote_value_usd = quote_balance * quote_price;
    let total_value_usd = base_value_usd + quote_value_usd;

    tracing::info!(
        "💰 {} Balances: {}: {:.6} (${:.2}) | {}: {:.6} (${:.2}) | Total: ${:.2}",
        prefix,
        target.base_token,
        base_balance,
        base_value_usd,
        target.quote_token,
        quote_balance,
        quote_value_usd,
        total_value_usd
    );

    Ok(())
}

// Check inventory balance for double leg mode
async fn check_inventory_balance<T: Network>(_provider: RootProvider<T>, target: &shd::types::ArbTarget, env: &EnvConfig, config: &BotConfig) -> Result<InventoryStatus>
where
    RootProvider<T>: Provider + Clone,
{
    // Get wallet for this target
    let wallet = match env.get_signer_for_address(&target.address) {
        Some(s) => s,
        None => {
            return Err(eyre::eyre!("No wallet found for target {}", target.vault_name));
        }
    };
    let wallet_address = wallet.address();

    // Fetch token balances
    let (base_decimals, quote_decimals, base_balance_raw, quote_balance_raw) =
        shd::utils::evm::get_token_info_and_balances(&config.global.rpc_endpoint, &format!("{:?}", wallet_address), &target.base_token_address, &target.quote_token_address)
            .await
            .map_err(|e| eyre::eyre!(e))?;

    let base_balance = base_balance_raw as f64 / 10f64.powi(base_decimals as i32);
    let quote_balance = quote_balance_raw as f64 / 10f64.powi(quote_decimals as i32);

    // Fetch current prices
    let base_price = fetch_price_by_reference(&target.reference, &target.base_token, config).await?;
    let quote_price = if target.quote_token.to_uppercase() == "USDT0" || target.quote_token.to_uppercase() == "USDC0" {
        1.0
    } else {
        fetch_price_by_reference(&target.reference, &target.quote_token, config).await?
    };

    // Calculate USD values
    let base_value_usd = base_balance * base_price;
    let quote_value_usd = quote_balance * quote_price;
    let total_value_usd = base_value_usd + quote_value_usd;

    // Calculate percentages
    let base_percentage = if total_value_usd > 0.0 { (base_value_usd / total_value_usd) * 100.0 } else { 0.0 };
    let quote_percentage = if total_value_usd > 0.0 { (quote_value_usd / total_value_usd) * 100.0 } else { 0.0 };

    // Check if inventory is balanced (both tokens between 20-80%)
    let is_balanced = base_percentage >= 20.0 && base_percentage <= 80.0 && quote_percentage >= 20.0 && quote_percentage <= 80.0;

    Ok(InventoryStatus {
        base_token: target.base_token.clone(),
        quote_token: target.quote_token.clone(),
        base_balance,
        quote_balance,
        base_percentage,
        quote_percentage,
        total_value_usd,
        is_balanced,
    })
}

// --- Main logic ---
async fn run<T: Network>(config: BotConfig, env: &EnvConfig, provider: RootProvider<T>, current_block: u64)
where
    RootProvider<T>: Provider + Clone,
{
    // For each vault
    for target in &config.targets {
        // Check inventory balance for double leg mode targets (every N blocks)
        // Do this BEFORE looking for opportunities to prevent execution if imbalanced
        if !target.statistical_arb && current_block % INVENTORY_CHECK_INTERVAL_BLOCKS == 0 {
            match check_inventory_balance(provider.clone(), &target, &env, &config).await {
                Ok(status) => {
                    if !status.is_balanced {
                        tracing::warn!(
                            "⚠️ INVENTORY IMBALANCE DETECTED in double-leg mode for {} (Block #{}):\n  \
                            {} balance: {:.6} ({:.1}% of total)\n  \
                            {} balance: {:.6} ({:.1}% of total)\n  \
                            Total value: ${:.2} USD\n  \
                            Skipping arbitrage - waiting for rebalancer algo to operate...",
                            target.vault_name,
                            current_block,
                            status.base_token,
                            status.base_balance,
                            status.base_percentage,
                            status.quote_token,
                            status.quote_balance,
                            status.quote_percentage,
                            status.total_value_usd
                        );
                        // Skip this target if inventory is imbalanced
                        continue;
                    } else {
                        tracing::info!(
                            "✅ Inventory balanced for {} (Block #{}): {} {:.1}% / {} {:.1}%",
                            target.vault_name,
                            current_block,
                            status.base_token,
                            status.base_percentage,
                            status.quote_token,
                            status.quote_percentage
                        );
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to check inventory balance for {}: {}. Skipping target.", target.vault_name, e);
                    continue;
                }
            }
        }

        tracing::info!("Monitoring target: {}", target.format_log_info());
        // Get the wallet signer for this vault address
        let wallet_signer = match env.get_signer_for_address(&target.address) {
            Some(signer) => signer,
            None => {
                tracing::error!("No matching wallet found for vault: {}. Skipping vault.", target.format_log_info());
                continue;
            }
        };
        let wallet_address = wallet_signer.address();
        // Verify the vault address matches the wallet address (lowercase comparison)
        if target.address.to_lowercase() != format!("0x{:x}", wallet_address).to_lowercase() {
            tracing::error!("Target address mismatch for {}: expected 0x{:x} | Skipping target ...", target.format_log_info(), wallet_address);
            continue;
        }

        // Fetch reference price for base in quote token
        let reference_price = match fetch_price_by_reference(&target.reference, &target.base_token, &config).await {
            Ok(price) => {
                tracing::info!("{}/{} Reference price from {:?}: ${:.2}", target.base_token, target.quote_token, target.reference, price);
                price
            }
            Err(e) => {
                tracing::warn!("Failed to fetch {} price from {:?}: {}", target.base_token, target.reference, e);
                continue; // Skip this target if we can't get the base price
            }
        };

        // Track the single best opportunity across all pools
        // (dex, pool, price, spread_bps, fee_bps, net_profit_bps, pool_fee_tier)
        let mut best_opportunity: Option<(String, String, f64, f64, f64, f64, u32)> = None;

        // For double-leg arb: track all opportunities
        let mut all_opportunities: Vec<shd::dex::swap::BestOpportunity> = Vec::new();

        // >>>>> Hyperswap pools <<<<<
        // tracing::info!("Hyperswap Pools:");
        for pool_addr_str in &target.hyperswap_pools {
            if pool_addr_str.is_empty() {
                continue;
            }

            if let Ok(pool_addr) = Address::from_str(pool_addr_str) {
                match get_pool_info(provider.clone(), pool_addr).await {
                    Ok(pool_info) => {
                        let price = calculate_pool_prices(&pool_info);

                        // Determine which price to use based on token order
                        // We need the price of base_token in terms of quote_token
                        let pool_price = if pool_info.token0.to_string().to_lowercase() == target.base_token_address.to_lowercase() {
                            // base_token is token0, so we want token0/token1 price
                            price.token0_price
                        } else if pool_info.token1.to_string().to_lowercase() == target.base_token_address.to_lowercase() {
                            // base_token is token1, so we want token1/token0 price
                            price.token1_price
                        } else {
                            tracing::warn!("Pool {} doesn't contain base token {}", &pool_addr_str[..10], target.base_token);
                            continue;
                        };

                        let spread_bps = ((pool_price - reference_price) / reference_price) * BASIS_POINT_DENO;
                        let fee_bps = (price.fee as f64) / 100.0; // Convert fee to basis points
                        let net_profit_bps = spread_bps.abs() - fee_bps; // Single fee for one-way trade

                        tracing::debug!(
                            " - {} | Pool: ${:.2} | Ref: ${:.2} | Spread: {:.1} bps | Net of pool fees: {:.1} bps",
                            &pool_addr_str[..10],
                            pool_price,
                            reference_price,
                            spread_bps,
                            net_profit_bps
                        );

                        // Update best opportunity if this pool is better
                        // Use min_executable_spread_bps as threshold (can be negative for lossy trades)
                        if net_profit_bps >= target.min_executable_spread_bps && spread_bps.abs() >= target.min_watch_spread_bps {
                            if best_opportunity.is_none() || net_profit_bps > best_opportunity.as_ref().unwrap().5 {
                                best_opportunity = Some(("Hyperswap".to_string(), pool_addr_str.clone(), pool_price, spread_bps, fee_bps, net_profit_bps, price.fee));
                            }
                        }

                        // For double-leg: collect all opportunities
                        if !target.statistical_arb && target.reference == PriceReference::Hypercore {
                            all_opportunities.push(shd::dex::swap::BestOpportunity {
                                dex: "Hyperswap".to_string(),
                                pool_address: pool_addr_str.clone(),
                                pool_price,
                                spread_bps,
                                fee_bps,
                                net_profit_bps,
                                pool_fee_tier: price.fee,
                            });
                        }
                    }
                    Err(e) => {
                        tracing::debug!("    Failed to fetch {}: {}", &pool_addr_str[..10], e);
                    }
                }
            }
        }

        // >>>>> ProjectX pools <<<<<
        // tracing::info!("ProjectX Pools:");
        for pool_addr_str in &target.prjx_pools {
            if pool_addr_str.is_empty() {
                continue;
            }

            if let Ok(pool_addr) = Address::from_str(pool_addr_str) {
                match get_pool_info(provider.clone(), pool_addr).await {
                    Ok(pool_info) => {
                        let price = calculate_pool_prices(&pool_info);

                        // Determine which price to use based on token order
                        // We need the price of base_token in terms of quote_token
                        let pool_price = if pool_info.token0.to_string().to_lowercase() == target.base_token_address.to_lowercase() {
                            // base_token is token0, so we want token0/token1 price
                            price.token0_price
                        } else if pool_info.token1.to_string().to_lowercase() == target.base_token_address.to_lowercase() {
                            // base_token is token1, so we want token1/token0 price
                            price.token1_price
                        } else {
                            tracing::warn!("Pool {} doesn't contain base token {}", &pool_addr_str[..10], target.base_token);
                            continue;
                        };

                        let spread_bps = ((pool_price - reference_price) / reference_price) * BASIS_POINT_DENO;
                        let fee_bps = (price.fee as f64) / 100.0; // Convert fee to basis points
                        let net_profit_bps = spread_bps.abs() - fee_bps; // Single fee for one-way trade

                        tracing::debug!(
                            " - {} | Pool: ${:.2} | Ref: ${:.2} | Spread: {:.1} bps | Net of pool fees: {:.1} bps",
                            &pool_addr_str[..10],
                            pool_price,
                            reference_price,
                            spread_bps,
                            net_profit_bps
                        );

                        // Update best opportunity if this pool is better
                        // Use min_executable_spread_bps as threshold (can be negative for lossy trades)
                        if net_profit_bps >= target.min_executable_spread_bps && spread_bps.abs() >= target.min_watch_spread_bps {
                            if best_opportunity.is_none() || net_profit_bps > best_opportunity.as_ref().unwrap().5 {
                                best_opportunity = Some(("ProjectX".to_string(), pool_addr_str.clone(), pool_price, spread_bps, fee_bps, net_profit_bps, price.fee));
                            }
                        }

                        // For double-leg: collect all opportunities
                        if !target.statistical_arb && target.reference == PriceReference::Hypercore {
                            all_opportunities.push(shd::dex::swap::BestOpportunity {
                                dex: "ProjectX".to_string(),
                                pool_address: pool_addr_str.clone(),
                                pool_price,
                                spread_bps,
                                fee_bps,
                                net_profit_bps,
                                pool_fee_tier: price.fee,
                            });
                        }
                    }
                    Err(e) => {
                        tracing::debug!("   - Failed to fetch {}: {}", &pool_addr_str[..10], e);
                    }
                }
            }
        }

        // Process the best opportunity if we found one
        if let Some((dex, pool, price, spread, fee, net_profit, pool_fee_tier)) = best_opportunity {
            tracing::info!("Best opportunity found:");
            tracing::info!(
                "{} {} | ${:.4} | Spread: {:.2} bps | Fee: {:.2} bps | Net of pool fees: {:.2} bps",
                dex,
                &pool[..10],
                price,
                spread,
                fee,
                net_profit
            );

            // Check if net profit exceeds executable threshold
            if net_profit >= target.min_executable_spread_bps {
                tracing::info!("Exceeds executable threshold ({} bps) - Ready to execute", target.min_executable_spread_bps);
                // If statistical_arb is true : just buy/sell accordingly
                if target.statistical_arb {
                    tracing::info!("📈 Statistical arbitrage mode - executing trade");

                    // Log current balances before trade
                    if let Err(e) = log_current_balances(provider.clone(), &target, &env, &config, "Pre-Trade").await {
                        tracing::error!("Failed to log pre-trade balances: {}", e);
                    }

                    // Create BestOpportunity struct
                    let opportunity = shd::dex::swap::BestOpportunity {
                        dex: dex.clone(),
                        pool_address: pool.clone(),
                        pool_price: price,
                        spread_bps: spread,
                        fee_bps: fee,
                        net_profit_bps: net_profit,
                        pool_fee_tier,
                    };

                    // Execute the swap
                    match shd::dex::swap::execute_statistical_arbitrage(provider.clone(), opportunity, &target, &env, &config, reference_price).await {
                        Ok(_) => {
                            tracing::info!("Trade executed successfully");
                            // Log new balances after trade
                            if let Err(e) = log_current_balances(provider.clone(), &target, &env, &config, "Post-Trade").await {
                                tracing::error!("Failed to log post-trade balances: {}", e);
                            }
                        }
                        Err(e) => tracing::error!("Trade execution failed: {}", e),
                    }
                } else if target.reference == PriceReference::Hypercore {
                    // Double-leg arbitrage mode (only for Hypercore reference)
                    tracing::info!("🔄 Double-leg arbitrage mode - preparing parameters");

                    // Find best buy opportunity (lowest price) and sell opportunity (highest price)
                    let buy_opp = all_opportunities.iter().min_by(|a, b| a.pool_price.partial_cmp(&b.pool_price).unwrap());
                    let sell_opp = all_opportunities.iter().max_by(|a, b| a.pool_price.partial_cmp(&b.pool_price).unwrap());

                    if let (Some(buy), Some(sell)) = (buy_opp, sell_opp) {
                        // Only proceed if there's a profitable spread
                        if sell.pool_price > buy.pool_price {
                            let spread_profit = ((sell.pool_price - buy.pool_price) / buy.pool_price) * BASIS_POINT_DENO;
                            let total_fees = buy.fee_bps + sell.fee_bps;
                            let net_profit = spread_profit - total_fees;

                            if net_profit >= target.min_executable_spread_bps {
                                tracing::info!("Found profitable double-leg opportunity:");
                                tracing::info!("  Buy on {} at ${:.4}", buy.dex, buy.pool_price);
                                tracing::info!("  Sell on {} at ${:.4}", sell.dex, sell.pool_price);
                                tracing::info!("  Spread: {:.2} bps | Fees: {:.2} bps | Net of pool fees: {:.2} bps", spread_profit, total_fees, net_profit);

                                // Prepare double-leg arbitrage
                                match shd::dex::swap_double_leg::prepare_double_leg_arbitrage(provider.clone(), buy.clone(), sell.clone(), &target, &env, &config, reference_price).await {
                                    Ok((pool_swap, spot_order, double_leg)) => {
                                        tracing::info!("✅ Double-leg arbitrage prepared successfully");
                                        tracing::info!("Pool swap params: {:?}", pool_swap);
                                        tracing::info!("Spot order params: {:?}", spot_order);
                                        tracing::info!("Expected profit: ${:.2}", double_leg.expected_profit_usd);

                                        // Log current balances before execution
                                        if let Err(e) = log_current_balances(provider.clone(), &target, &env, &config, "Pre-Double-Leg").await {
                                            tracing::error!("Failed to log pre-trade balances: {}", e);
                                        }

                                        // Contract will use these params to execute atomically
                                        // NOTE: When contract execution is implemented, add post-trade balance logging here
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to prepare double-leg arbitrage: {}", e);
                                    }
                                }
                            } else {
                                tracing::info!("Double-leg net profit ({:.2} bps) below threshold", net_profit);
                            }
                        }
                    } else {
                        tracing::info!("No double-leg opportunities found");
                    }
                } else {
                    tracing::info!("Double-leg arbitrage only supported with Hypercore reference");
                }
            } else {
                tracing::info!("Net profit ({:.2} bps) below executable threshold ({} bps)", net_profit, target.min_executable_spread_bps);
            }
        } else {
            tracing::info!(
                "No pools found meeting criteria (net profit >= {} bps and spread >= {} bps)",
                target.min_executable_spread_bps,
                target.min_watch_spread_bps
            );
        }
        tracing::info!("Completed checks for target: {}", target.format_log_info());
    }
}

/// Main monitoring function that checks for new events and updates reserves
async fn moni<T: Network>(config: BotConfig, env: EnvConfig, provider: RootProvider<T>)
where
    RootProvider<T>: Provider + Clone,
{
    let mut last: Option<u64> = None;
    let mut time = std::time::SystemTime::now();
    let interval = 250;
    let mut _loop_count = 0u64; // Track number of loops for testing (currently unused)
    tracing::info!("Starting monitoring with interval: {} ms", interval);
    loop {
        match provider.get_block_number().await {
            Ok(current) => match last {
                Some(prev) => {
                    // --- Fetch new logs ---
                    let elapsed = time.elapsed().unwrap().as_millis() as u64;
                    if elapsed > interval && current > prev {
                        let delta = current - prev;
                        tracing::info!("💎 New block range: [{}, {}] with a delta of {} blocks", prev, current, delta);
                        // --- Main logic ---
                        let _res = run(config.clone(), &env, provider.clone(), current).await;
                        // --- End Main logic ---
                        last = Some(current);
                        time = std::time::SystemTime::now();
                    }
                }
                None => {
                    tracing::info!("Starting block number: {}", current);
                    last = Some(current);
                }
            },
            Err(e) => {
                tracing::error!("Error fetching block number: {}", e);
            }
        }
        time::sleep(std::time::Duration::from_millis(interval)).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing subscriber and load configurations
    let filter = EnvFilter::from_default_env();
    fmt().with_max_level(Level::TRACE).with_env_filter(filter).init();
    dotenv::from_filename("config/.env").ok();
    let env = EnvConfig::new();
    // let path = "config/main.demo.toml";
    let path = "config/main.toml"; // ! @PROD
    tracing::info!("Loading bot configuration from: {}", path);
    let config = load_bot_config_with_env(path, &env);

    // Log the initialization
    tracing::info!("🔑 Multi-wallet system initialized with {} wallets", env.wallet_pub_keys.len());

    // Log target configurations
    tracing::info!("📊 Configured {} arbitrage targets:", config.targets.len());
    for target in &config.targets {
        let hyperswap_count = target.hyperswap_pools.iter().filter(|p| !p.is_empty()).count();
        let prjx_count = target.prjx_pools.iter().filter(|p| !p.is_empty()).count();
        tracing::info!(
            "• {} ({}/{}) - Address: {}, Pools: {} Hyperswap, {} ProjectX",
            target.vault_name,
            target.base_token,
            target.quote_token,
            if target.address.len() > 10 { &target.address[..10] } else { &target.address },
            hyperswap_count,
            prjx_count
        );
        tracing::info!(
            "Spreads: watch={} bps, exec={} bps | Slippage: {}% | Poll: {}ms",
            target.min_watch_spread_bps,
            target.min_executable_spread_bps,
            target.max_slippage_pct * 100.0,
            target.poll_interval_ms
        );
        tracing::info!("Reference: {} | Statistical Arb: {}", target.reference, if target.statistical_arb { "Yes (EVM-only)" } else { "No" });
    }

    // Build HTTP provider using network's RPC
    let provider = match config.global.rpc_endpoint.parse() {
        Ok(parsed) => RootProvider::<Ethereum>::new_http(parsed),
        Err(e) => {
            tracing::error!("Failed to parse RPC URL: {}", e);
            return Ok(());
        }
    };
    let current = shd::utils::misc::block(provider.clone()).await.unwrap();
    tracing::info!("🚀 Launching monitoring, starting at block #{}", current);

    // Fetch prices for all configured targets based on their oracle reference
    tracing::info!("📊 Fetching prices for all configured targets...");
    let mut hype_price = 0.0;

    // Initialize spot balance fetcher
    let spot_fetcher = match shd::core::spot::HyperliquidSpotBalances::new() {
        Ok(fetcher) => Some(fetcher),
        Err(e) => {
            tracing::warn!("Failed to initialize spot balance fetcher: {}", e);
            None
        }
    };

    // Log balances for each target
    tracing::info!("💰 Checking balances for all targets...");
    for target in &config.targets {
        // Get wallet for this target
        let wallet = match env.get_signer_for_address(&target.address) {
            Some(s) => s,
            None => {
                tracing::error!("No wallet found for target {}", target.vault_name);
                continue;
            }
        };
        let wallet_address = wallet.address();

        // Fetch EVM balances
        if !target.base_token_address.is_empty() && !target.quote_token_address.is_empty() {
            match shd::utils::evm::get_token_info_and_balances(&config.global.rpc_endpoint, &format!("{:?}", wallet_address), &target.base_token_address, &target.quote_token_address).await {
                Ok((base_decimals, quote_decimals, base_balance, quote_balance)) => {
                    let base_balance_formatted = base_balance as f64 / 10f64.powi(base_decimals as i32);
                    let quote_balance_formatted = quote_balance as f64 / 10f64.powi(quote_decimals as i32);

                    tracing::info!(
                        "  {} ({}) EVM - {}: {:.6} | {}: {:.6}",
                        target.vault_name,
                        &target.address[..10],
                        target.base_token,
                        base_balance_formatted,
                        target.quote_token,
                        quote_balance_formatted
                    );
                }
                Err(e) => {
                    tracing::error!("Failed to fetch EVM balances for {}: {}", target.vault_name, e);
                }
            }
        }

        // Fetch Hyperliquid spot balances if fetcher is available
        if let Some(ref fetcher) = spot_fetcher {
            match fetcher.get_non_zero_balances(&format!("{:?}", wallet_address)).await {
                Ok(spot_balances) => {
                    if !spot_balances.is_empty() {
                        let mut spot_summary = Vec::new();
                        for balance in spot_balances.iter().take(5) {
                            // Show max 5 tokens
                            if let Ok(total) = balance.total_as_f64() {
                                spot_summary.push(format!("{}: {:.4}", balance.coin, total));
                            }
                        }
                        tracing::info!("  {} ({}) HyperliquidSpot - {}", target.vault_name, &target.address[..10], spot_summary.join(" | "));
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to fetch Hyperliquid spot balances for {}: {}", target.vault_name, e);
                }
            }
        }
    }

    for target in &config.targets {
        tracing::info!("Fetching prices for {} using {:?} oracle", target.vault_name, target.reference);

        // Fetch base token price
        match fetch_price_by_reference(&target.reference, &target.base_token, &config).await {
            Ok(price) => {
                tracing::info!("💰 {}/{} Price from {:?}: ${:.2}", target.base_token, "USD", target.reference, price);
                if target.base_token.to_uppercase() == "HYPE" || target.base_token.to_uppercase() == "WHYPE" {
                    hype_price = price;
                }
            }
            Err(e) => {
                if target.base_token.to_uppercase() == "USDT0" || target.base_token.to_uppercase() == "USDC0" {
                    // tracing::debug!("{} is a stablecoin, using $1.00", target.base_token);
                } else {
                    tracing::warn!("Failed to fetch {} price from {:?}: {}", target.base_token, target.reference, e);
                }
            }
        }

        // Fetch quote token price if different from base
        if target.quote_token != target.base_token {
            match fetch_price_by_reference(&target.reference, &target.quote_token, &config).await {
                Ok(price) => {
                    tracing::info!("💰 {}/{} Price from {:?}: ${:.2}", target.quote_token, "USD", target.reference, price);
                    if target.quote_token.to_uppercase() == "HYPE" || target.quote_token.to_uppercase() == "WHYPE" {
                        hype_price = price;
                    }
                }
                Err(e) => {
                    if target.quote_token.to_uppercase() == "USDT0" || target.quote_token.to_uppercase() == "USDC0" {
                        // tracing::debug!("{} is a stablecoin, using $1.00", target.quote_token);
                    } else {
                        tracing::warn!("Failed to fetch {} price from {:?}: {}", target.quote_token, target.reference, e);
                    }
                }
            }
        }
    }

    // Log gas prices if we have HYPE price
    if hype_price > 0.0 {
        let (_, _gas_gwei, _) = log_gas_prices(provider.clone(), hype_price).await.unwrap();
    } else {
        tracing::warn!("HYPE price not available, skipping gas price calculation");
    }

    init_allowance(&config, &env).await;

    // Spawn a Tokio task that polls the block number
    let handle = task::spawn(async move {
        let _config = config.clone();
        let _provider = provider.clone();
        let _env = env.clone();
        moni(_config, _env, _provider).await;
    });
    // Await the polling task (never returns under normal operation)
    match handle.await {
        Ok(_) => tracing::info!("Polling task finished unexpectedly"),
        Err(e) => tracing::error!("Polling task panicked: {}", e),
    }
    Ok(())
}
