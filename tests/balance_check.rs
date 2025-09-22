use eyre::Result;
use shd::{
    core::spot::HyperliquidSpotBalances,
    types::{EnvConfig, load_bot_config_with_env},
    utils::evm::get_token_info_and_balances,
};

#[tokio::test]
async fn test_fetch_all_balances() -> Result<()> {
    // Load environment and config
    dotenv::from_filename("config/.env").ok();
    let env = EnvConfig::new();
    let config_path = "config/main.toml";
    let config = load_bot_config_with_env(config_path, &env);

    println!("\n===========================================");
    println!("BALANCE CHECK FOR ALL CONFIGURED TARGETS");
    println!("===========================================\n");

    // Initialize spot balance fetcher
    let spot_fetcher = HyperliquidSpotBalances::new()?;

    // For each target in config
    for target in &config.targets {
        println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Target: {} ({})", target.vault_name, target.address);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

        // Get wallet for this target
        let wallet = match env.get_signer_for_address(&target.address) {
            Some(s) => s,
            None => {
                println!("❌ No wallet found for this target, skipping...");
                continue;
            }
        };
        let wallet_address = wallet.address();

        // ===== HYPERLIQUID SPOT BALANCES =====
        println!("📊 HYPERLIQUID SPOT BALANCES:");
        println!("   Address: 0x{:x}", wallet_address);

        match spot_fetcher.get_spot_balances(&format!("0x{:x}", wallet_address)).await {
            Ok(spot_balances) => {
                if spot_balances.is_empty() {
                    println!("   No spot balances found");
                } else {
                    println!("   {:<10} {:>20} {:>20} {:>20}", "Token", "Total", "Hold", "Available");
                    println!("   {}", "-".repeat(75));

                    for balance in spot_balances.iter().take(10) {
                        // Show max 10 tokens
                        // Only show non-zero balances
                        if let Ok(total) = balance.total_as_f64() {
                            if total > 0.0 {
                                println!(
                                    "   {:<10} {:>20} {:>20} {:>20}",
                                    balance.coin,
                                    balance.total,
                                    balance.hold,
                                    balance.available.as_ref().unwrap_or(&"N/A".to_string())
                                );
                            }
                        }
                    }
                }
            }
            Err(e) => {
                println!("   ❌ Error fetching spot balances: {}", e);
            }
        }

        // ===== EVM BALANCES =====
        println!("\n💎 HYPEREVM BALANCES:");
        
        // Check if token addresses are configured (some targets like BTC/ETH don't have EVM addresses)
        if target.base_token_address.is_empty() || target.quote_token_address.is_empty() {
            println!("   ⚠️  No EVM token addresses configured for this target");
            println!("   Base Token: {}", target.base_token);
            println!("   Quote Token: {}", target.quote_token);
        } else {
            println!("   Base Token: {} ({})", target.base_token, &target.base_token_address[..10]);
            println!("   Quote Token: {} ({})", target.quote_token, &target.quote_token_address[..10]);

            match get_token_info_and_balances(&config.global.rpc_endpoint, &format!("0x{:x}", wallet_address), &target.base_token_address, &target.quote_token_address).await {
            Ok((base_decimals, quote_decimals, base_balance_raw, quote_balance_raw)) => {
                let base_balance = base_balance_raw as f64 / 10f64.powi(base_decimals as i32);
                let quote_balance = quote_balance_raw as f64 / 10f64.powi(quote_decimals as i32);

                println!("\n   {:<15} {:>20} {:>15}", "Token", "Balance", "Decimals");
                println!("   {}", "-".repeat(52));

                if base_balance > 0.0 || quote_balance > 0.0 {
                    if base_balance > 0.0 {
                        println!("   {:<15} {:>20.6} {:>15}", target.base_token, base_balance, base_decimals);
                    }
                    if quote_balance > 0.0 {
                        println!("   {:<15} {:>20.6} {:>15}", target.quote_token, quote_balance, quote_decimals);
                    }
                } else {
                    println!("   No EVM balances found");
                }

                // Show total value if we have prices
                if target.base_token.to_uppercase().contains("HYPE") || target.quote_token.to_uppercase().contains("USD") {
                    println!("\n   📈 Estimated Values:");
                    if base_balance > 0.0 && target.base_token.to_uppercase().contains("HYPE") {
                        // Rough estimate assuming HYPE ~$30 (you can fetch actual price)
                        println!("   {}: ~${:.2} USD (estimate)", target.base_token, base_balance * 30.0);
                    }
                    if quote_balance > 0.0 && target.quote_token.to_uppercase().contains("USD") {
                        println!("   {}: ${:.2} USD", target.quote_token, quote_balance);
                    }
                }
            }
            Err(e) => {
                println!("   ❌ Error fetching EVM balances: {}", e);
            }
        }
        }

        // ===== COMPARISON =====
        println!("\n🔄 CROSS-LAYER SUMMARY:");

        // Try to find matching tokens between L1 and L2
        match spot_fetcher
            .get_specific_balances(
                &format!("0x{:x}", wallet_address),
                &[
                    &target.base_token.replace("wHYPE", "HYPE").replace("WHYPE", "HYPE"),
                    &target.quote_token.replace("USDT0", "USDT").replace("USDC0", "USDC"),
                ],
            )
            .await
        {
            Ok(matching_spot) => {
                for spot_balance in matching_spot {
                    if let Ok(total) = spot_balance.total_as_f64() {
                        if total > 0.0 {
                            println!("   {} on Hyperliquid Spot: {}", spot_balance.coin, spot_balance.total);
                        }
                    }
                }
            }
            Err(_) => {}
        }

        // Show configured pools
        let hyperswap_count = target.hyperswap_pools.iter().filter(|p| !p.is_empty()).count();
        let prjx_count = target.prjx_pools.iter().filter(|p| !p.is_empty()).count();
        println!("\n📍 CONFIGURED POOLS:");
        println!("   Hyperswap: {} pools", hyperswap_count);
        println!("   ProjectX: {} pools", prjx_count);
        println!("   Min watch spread: {} bps", target.min_watch_spread_bps);
        println!("   Min executable spread: {} bps", target.min_executable_spread_bps);
        println!("   Statistical arbitrage: {}", if target.statistical_arb { "Yes" } else { "No" });
    }

    println!("\n===========================================");
    println!("BALANCE CHECK COMPLETE");
    println!("===========================================\n");

    Ok(())
}

#[tokio::test]
async fn test_spot_balance_details() -> Result<()> {
    // Load environment
    dotenv::from_filename("config/.env").ok();
    let env = EnvConfig::new();
    let config_path = "config/main.toml";
    let config = load_bot_config_with_env(config_path, &env);

    if config.targets.is_empty() {
        println!("No targets configured");
        return Ok(());
    }

    println!("\n===========================================");
    println!("DETAILED SPOT BALANCE CHECK FOR ALL WALLETS");
    println!("===========================================");
    
    let spot_fetcher = HyperliquidSpotBalances::new()?;

    // Check ALL targets, not just the first one
    for target in &config.targets {
        // Get wallet
        let wallet = match env.get_signer_for_address(&target.address) {
            Some(s) => s,
            None => {
                println!("No wallet found for target: {}", target.vault_name);
                continue;
            }
        };
        let wallet_address = format!("0x{:x}", wallet.address());

        println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("Target: {} ({})", target.vault_name, &target.address[..10]);
        println!("Address: {}", wallet_address);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Get ALL spot balances (not just main tokens)
    match spot_fetcher.get_spot_balances(&wallet_address).await {
        Ok(all_balances) => {
            println!("Total tokens found: {}\n", all_balances.len());

            // Separate by balance status
            let mut with_balance = Vec::new();
            let mut without_balance = Vec::new();

            for balance in all_balances {
                if let Ok(total) = balance.total_as_f64() {
                    if total > 0.0 {
                        with_balance.push(balance);
                    } else {
                        without_balance.push(balance);
                    }
                }
            }

            if !with_balance.is_empty() {
                println!("TOKENS WITH BALANCE:");
                println!("{:<15} {:>20} {:>20} {:>20}", "Token", "Total", "Hold", "Available");
                println!("{}", "=".repeat(80));

                for balance in with_balance {
                    println!(
                        "{:<15} {:>20} {:>20} {:>20}",
                        balance.coin,
                        balance.total,
                        balance.hold,
                        balance.available.as_ref().unwrap_or(&"N/A".to_string())
                    );
                }
            } else {
                println!("No balances found for this wallet");
            }
        }
        Err(e) => {
            println!("Error fetching balances: {}", e);
        }
    }
    } // End of for loop over all targets

    println!("\n===========================================");
    println!("DETAILED CHECK COMPLETE FOR ALL {} WALLETS", config.targets.len());
    println!("===========================================\n");

    Ok(())
}
