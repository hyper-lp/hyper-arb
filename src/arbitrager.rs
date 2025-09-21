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

// --- Main logic ---
async fn run<T: Network>(config: BotConfig, env: &EnvConfig, provider: RootProvider<T>)
where
    RootProvider<T>: Provider + Clone,
{
    // For each vault
    for target in &config.targets {
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
                        let spread_bps = ((price.token0_price - reference_price) / reference_price) * BASIS_POINT_DENO;
                        let fee_bps = (price.fee as f64) / 100.0; // Convert fee to basis points
                        let net_profit_bps = spread_bps.abs() - fee_bps; // Single fee for one-way trade

                        tracing::info!(
                            " - {} | Pool: ${:.4} | Ref: ${:.4} | Spread: {:.2} bps | Fee: {:.2} bps | Net: {:.2} bps",
                            &pool_addr_str[..10],
                            price.token0_price,
                            reference_price,
                            spread_bps,
                            fee_bps,
                            net_profit_bps
                        );

                        // Update best opportunity if this pool is better
                        if net_profit_bps > 0.0 && spread_bps.abs() >= target.min_watch_spread_bps {
                            if best_opportunity.is_none() || net_profit_bps > best_opportunity.as_ref().unwrap().5 {
                                best_opportunity = Some(("Hyperswap".to_string(), pool_addr_str.clone(), price.token0_price, spread_bps, fee_bps, net_profit_bps, price.fee));
                            }
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
                        let spread_bps = ((price.token0_price - reference_price) / reference_price) * BASIS_POINT_DENO;
                        let fee_bps = (price.fee as f64) / 100.0; // Convert fee to basis points
                        let net_profit_bps = spread_bps.abs() - fee_bps; // Single fee for one-way trade

                        tracing::info!(
                            " - {} | Pool: ${:.4} | Ref: ${:.4} | Spread: {:.2} bps | Fee: {:.2} bps | Net: {:.2} bps",
                            &pool_addr_str[..10],
                            price.token0_price,
                            reference_price,
                            spread_bps,
                            fee_bps,
                            net_profit_bps
                        );

                        // Update best opportunity if this pool is better
                        if net_profit_bps > 0.0 && spread_bps.abs() >= target.min_watch_spread_bps {
                            if best_opportunity.is_none() || net_profit_bps > best_opportunity.as_ref().unwrap().5 {
                                best_opportunity = Some(("ProjectX".to_string(), pool_addr_str.clone(), price.token0_price, spread_bps, fee_bps, net_profit_bps, price.fee));
                            }
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
            tracing::info!("{} {} | ${:.4} | Spread: {:.2} bps | Fee: {:.2} bps | Net: {:.2} bps", dex, &pool[..10], price, spread, fee, net_profit);

            // Check if net profit exceeds executable threshold
            if net_profit >= target.min_executable_spread_bps.abs() {
                tracing::info!("Exceeds executable threshold ({} bps) - Ready to execute", target.min_executable_spread_bps.abs());
                // If statistical_arb is true : just buy/sell accordingly
                if target.statistical_arb {
                    tracing::info!("📈 Statistical arbitrage mode - executing trade");

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
                        Ok(_) => tracing::info!("✅ Trade executed successfully"),
                        Err(e) => tracing::error!("❌ Trade execution failed: {}", e),
                    }
                } else {
                    // If statistical_arb is false : prepare the order to be exec in the contract
                    tracing::info!("  📝 Contract double-leg arbitrage mode - would prepare contract order");
                    // TODO: Implement double-leg arb preparation
                }
            } else {
                tracing::info!("Net profit ({:.2} bps) below executable threshold ({} bps)", net_profit, target.min_executable_spread_bps.abs());
            }
        } else {
            tracing::info!("No profitable pools found (all have negative net profit after fees)");
        }
        tracing::info!("✅ Completed checks for target: {}", target.format_log_info());
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
                        let _res = run(config.clone(), &env, provider.clone()).await;
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

        // Fetch decimals and balances
        match shd::utils::evm::get_token_info_and_balances(&config.global.rpc_endpoint, &format!("{:?}", wallet_address), &target.base_token_address, &target.quote_token_address).await {
            Ok((base_decimals, quote_decimals, base_balance, quote_balance)) => {
                let base_balance_formatted = base_balance as f64 / 10f64.powi(base_decimals as i32);
                let quote_balance_formatted = quote_balance as f64 / 10f64.powi(quote_decimals as i32);

                tracing::info!(
                    "  {} ({}) - {}: {:.6} | {}: {:.6}",
                    target.vault_name,
                    &target.address[..10],
                    target.base_token,
                    base_balance_formatted,
                    target.quote_token,
                    quote_balance_formatted
                );
            }
            Err(e) => {
                tracing::error!("Failed to fetch balances for {}: {}", target.vault_name, e);
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
                    tracing::debug!("{} is a stablecoin, using $1.00", target.base_token);
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
                        tracing::debug!("{} is a stablecoin, using $1.00", target.quote_token);
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
