use alloy::{
    network::{Ethereum, Network},
    providers::{Provider, RootProvider},
};
use eyre::Result;
use shd::{
    misc::log_gas_prices,
    types::{BotConfig, EnvConfig, load_bot_config_with_env},
};
use tokio::{task, time};
use tracing::Level;
use tracing_subscriber::{EnvFilter, fmt};

// --- Main logic ---
async fn run(config: BotConfig, env: &EnvConfig) {
    // For each vault
    for target in &config.targets {
        tracing::info!("🎯 Monitoring target: {}", target.format_log_info());

        // Get the wallet signer for this vault address
        let wallet_signer = match env.get_signer_for_address(&target.address) {
            Some(signer) => signer,
            None => {
                tracing::error!(
                    "No matching wallet found for vault: {}. Skipping vault.",
                    target.format_log_info()
                );
                continue;
            }
        };
        let wallet_address = wallet_signer.address();
        tracing::info!("Using wallet for vault: {}", target.format_log_info());
        // Verify the vault address matches the wallet address (lowercase comparison)
        if target.address.to_lowercase() != format!("0x{:x}", wallet_address).to_lowercase() {
            tracing::error!(
                "Target address mismatch for {}: expected 0x{:x} | Skipping target ...",
                target.format_log_info(),
                wallet_address
            );
            continue;
        }
    }
}

/// Main monitoring function that checks for new events and updates reserves
async fn moni<T: Network>(config: BotConfig, env: EnvConfig, provider: RootProvider<T>) {
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
                        tracing::info!(
                            "💎 New block range: [{}, {}] with a delta of {} blocks",
                            prev,
                            current,
                            delta
                        );
                        // --- Main logic ---
                        // let _res = run(config.clone(), &env).await;
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
    fmt()
        .with_max_level(Level::TRACE)
        .with_env_filter(filter)
        .init();
    dotenv::from_filename("config/.env").ok();
    let env = EnvConfig::new();
    // let path = "config/main.demo.toml";
    let path = "config/main.toml"; // ! @PROD
    tracing::info!("Loading bot configuration from: {}", path);
    let config = load_bot_config_with_env(path, &env);

    // Log the initialization
    tracing::info!(
        "🔑 Multi-wallet system initialized with {} wallets",
        env.wallet_pub_keys.len()
    );

    // Log target configurations
    tracing::info!("📊 Configured {} arbitrage targets:", config.targets.len());
    for target in &config.targets {
        tracing::info!(
            "• {} ({}/{}) - Address: {}, Pools: {} Hyperswap, {} ProjectX",
            target.vault_name,
            target.base_token,
            target.quote_token,
            if target.address.len() > 10 {
                &target.address[..10]
            } else {
                &target.address
            },
            target.hyperswap_pools.len(),
            target.prjx_pools.len()
        );
        tracing::info!(
            "Spreads: watch={} bps, exec={} bps | Slippage: {}% | Poll: {}ms",
            target.min_watch_spread_bps,
            target.min_executable_spread_bps,
            target.max_slippage_pct * 100.0,
            target.poll_interval_ms
        );
    }

    // Build HTTP provider using network's RPC
    let provider = match config.global.rpc_endpoint.parse() {
        Ok(parsed) => RootProvider::<Ethereum>::new_http(parsed),
        Err(e) => {
            tracing::error!("Failed to parse RPC URL: {}", e);
            return Ok(());
        }
    };
    let current = shd::misc::block(provider.clone()).await.unwrap();
    tracing::info!("🚀 Launching monitoring, starting at block #{}", current);

    // Fetch HYPE price from Pyth oracle
    match shd::oracles::fetch_hype_usd_price().await {
        Ok(hype_price) => {
            tracing::info!("💰 HYPE/USD Price from Pyth: ${:.2}", hype_price);
            let (_, _gas_gwei, _) = log_gas_prices(provider.clone(), hype_price).await.unwrap();
        }
        Err(e) => {
            tracing::warn!("Failed to fetch HYPE price from Pyth: {}", e);
            panic!("HYPE price is required for operation")
        }
    }

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
