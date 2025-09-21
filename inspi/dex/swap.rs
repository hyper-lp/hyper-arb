use alloy::{
    primitives::{Address, U256},
    providers::Provider,
};
use eyre::Result;
use std::str::FromStr;
use tokio::time::{Duration, sleep};
use tracing::{info, warn};

use chrono::Utc;

// Constants for transaction deadlines
const TRANSACTION_DEADLINE_SECONDS: i64 = 300; // 5 minutes

use crate::{
    dex::{
        liquidswap::swap_with_liquidswap,
        u3pos::{IERC20, IHyperSwapRouter, IProjectXRouter, RebalancingInfo, get_token_decimals_defaults},
    },
    misc::utils::format_tx_link,
    types::config::BotConfig,
};

/// Progress tracking enum - tracks each major step of the rebalancing process
/// Used for debugging and providing visibility into where failures occur
#[derive(Debug, Clone)]
#[allow(dead_code)] // Some variants used only for logging/debugging
pub enum RebalanceStep {
    Started,
    LiquidityDecreased { tx: String },
    FeesCollected { tx: String },
    ResidualsCleared { tx: Option<String> },
    NativeHypeWrapped { tx: Option<String> },
    NftBurned { tx: String },
    BalancesChecked { token0: U256, token1: U256 },
    Token0Swapped { tx: String, amount: U256, swap_method: String },
    Token1Swapped { tx: String, amount: U256, swap_method: String },
    TokensApproved { token0_tx: Option<String>, token1_tx: Option<String> },
    NewPositionMinted { token_id: u128, tx: String },
    Completed { new_token_id: u128 },
}

/// Parameters for performing token swaps during rebalancing
#[derive(Debug, Clone)]
pub struct SwapParams {
    pub config: BotConfig,
    pub wallet_address: Address,
    pub router_address: Address,
    pub token0: Address,
    pub token1: Address,
    pub fee: u32,
    pub balance0: U256,
    pub balance1: U256,
    pub rebalancing_info: RebalancingInfo,
    pub vault_config: crate::types::config::TrackingConfig,
    pub dex_name: String,
    pub adjusted_gas_price: u128,
}

/// Result from performing token swaps
#[derive(Debug, Clone)]
pub struct SwapResult {
    pub new_balance0: U256,
    pub new_balance1: U256,
    pub token0_swap_tx: Option<String>,
    pub token1_swap_tx: Option<String>,
    pub current_step: RebalanceStep,
}

/// Performs token swaps as part of the rebalancing process
///
/// ## Overview
/// This function handles STEP 5 of the rebalancing process - performing token swaps
/// to achieve the target token ratio for the new LP position:
///
/// 1. **Token0 to Token1 swap**: If we have excess token0
/// 2. **Token1 to Token0 swap**: If we have excess token1
/// 3. **Balance updates**: Fetches new balances after swaps
///
/// ## Parameters
/// - `provider`: Blockchain provider for transaction execution
/// - `params`: Swap parameters including addresses, amounts, and configuration
///
/// ## Returns
/// - `Ok(SwapResult)`: Updated balances and transaction details
/// - `Err(...)`: Error if any swap operation fails
///
/// ## Features
/// - **Automatic approval**: Checks and approves router spending if needed
/// - **Gas estimation**: Estimates and applies gas multiplier for transactions
/// - **Balance validation**: Ensures sufficient token balances before swapping
/// - **Detailed logging**: Comprehensive transaction and status logging
/// - **Error handling**: Graceful handling of insufficient balances
pub async fn perform_token_swaps<P: Provider + Clone>(provider: P, params: SwapParams) -> Result<SwapResult> {
    info!("Step 5: Performing token swaps for vault: {}", params.vault_config.format_log_info());

    let use_liquidswap = params.vault_config.use_dex_aggregator_for_swaps;
    let is_hyperswap = params.dex_name.to_lowercase() == "hyperswap";

    if use_liquidswap {
        info!("Using Liquid Labs DEX aggregator for optimal routing");
        // Note: router_address should be the Liquid Labs router when use_dex_aggregator_for_swaps is true
        info!("Liquid Labs router: {}", params.config.hyperevm.liqd_multi_hop_router_address);
    } else {
        info!("Using {} router: {}", params.dex_name, params.router_address);
        info!(
            "Router type: {} ({})",
            if is_hyperswap { "HyperSwap" } else { "ProjectX" },
            if is_hyperswap { "7 params, no deadline" } else { "8 params, with deadline" }
        );
    }

    let token0_contract = IERC20::new(params.token0, provider.clone());
    let token1_contract = IERC20::new(params.token1, provider.clone());

    info!("Pool tokens: {} -> {} (fee: {})", params.token1, params.token0, params.fee);

    let mut current_step = RebalanceStep::BalancesChecked {
        token0: params.balance0,
        token1: params.balance1,
    };
    let mut token0_swap_tx = None;
    let mut token1_swap_tx = None;

    // Token0 to Token1 swap
    if params.rebalancing_info.swap_amount_token0 > 0.0 {
        info!("Step 5a: Swapping {} Token0 to Token1", params.rebalancing_info.swap_amount_token0);

        // Convert float amount to raw token units (smallest denomination)
        let (decimals0, decimals1) = get_token_decimals_defaults(params.token0, params.token1);
        let swap_amount_raw = U256::from((params.rebalancing_info.swap_amount_token0 * 10_f64.powi(decimals0 as i32)) as u64);

        // Add balance validation with 1% buffer for safety
        let actual_balance0 = token0_contract.balanceOf(params.wallet_address).call().await?;
        let safe_swap_amount = if actual_balance0 > swap_amount_raw {
            swap_amount_raw // Use calculated amount if we have enough
        } else {
            // Use 99% of actual balance to leave buffer for gas
            let buffered_amount = actual_balance0 * U256::from(99) / U256::from(100);
            warn!(
                "Calculated swap amount {} exceeds balance {}, using buffered amount {}",
                swap_amount_raw, actual_balance0, buffered_amount
            );
            buffered_amount
        };

        // Check if we have enough token0 to swap (after buffer)
        if safe_swap_amount > U256::ZERO && safe_swap_amount <= actual_balance0 {
            // Determine which router to approve based on whether we're using Liquid Labs
            let approval_router = if use_liquidswap {
                // Use Liquid Labs router for approval
                Address::from_str(&params.config.hyperevm.liqd_multi_hop_router_address)?
            } else {
                // Use the DEX-specific router
                params.router_address
            };

            // Approve router to spend token0
            if token0_contract.allowance(params.wallet_address, approval_router).call().await? < safe_swap_amount {
                let approve_call = token0_contract.approve(approval_router, safe_swap_amount);
                let approve_gas_estimate = approve_call.estimate_gas().await?;
                let approve_gas_with_buffer = ((approve_gas_estimate as f64) * params.config.gas.gas_estimate_multiplier) as u64;
                info!(
                    "Gas estimate for token0 approve: {} (with {}x multiplier: {})",
                    approve_gas_estimate, params.config.gas.gas_estimate_multiplier, approve_gas_with_buffer
                );

                let nonce = provider.get_transaction_count(params.wallet_address).await?;
                info!("📤 Sending token0 swap approval with nonce: {}", nonce);
                sleep(Duration::from_millis(params.config.gas.nonce_delay_ms)).await;

                let pending_tx = token0_contract
                    .approve(approval_router, safe_swap_amount)
                    .gas(approve_gas_with_buffer)
                    .gas_price(params.adjusted_gas_price)
                    .send()
                    .await?;
                let tx_hash = pending_tx.tx_hash();
                info!("Token0 approval for swap, tx sent: {}", tx_hash);
                info!("🔗 {}", format_tx_link(&params.config.global.explorer_base_url, &tx_hash.to_string()));

                let approve_tx = pending_tx.watch().await?;
                info!("Token0 approved for swap, confirmed: {}", approve_tx);
            }

            // Calculate minimum output amount using actual pool price (95% slippage protection)
            let current_price = params.rebalancing_info.current_price; // USDT per WHYPE from pool

            // Convert swap amount from raw WHYPE units to human readable (use safe amount)
            let swap_amount_tokens = safe_swap_amount.to_string().parse::<f64>().unwrap_or(0.0) / 10_f64.powi(decimals0 as i32);

            // Calculate expected USDT output in human readable terms
            let expected_usdt_tokens = swap_amount_tokens * current_price;

            // Convert back to raw USDT units
            let expected_output = U256::from((expected_usdt_tokens * 10_f64.powi(decimals1 as i32)) as u64);
            let amount_out_minimum = expected_output * U256::from(95) / U256::from(100); // 5% slippage

            info!("Swap parameters debug:");
            info!("  tokenIn (token0): {}", params.token0);
            info!("  tokenOut (token1): {}", params.token1);
            info!("  fee: {}", params.fee);
            info!("  pool price: {:.6} USDT per WHYPE", current_price);
            info!("  amountIn: {} raw units ({:.6} tokens)", safe_swap_amount, swap_amount_tokens);
            info!("  expected output: {:.6} USDT tokens", expected_usdt_tokens);
            info!("  amountOutMinimum: {} raw units (95% of expected)", amount_out_minimum);

            // Execute swap with DEX-specific router or Liquid Labs aggregator
            let swap_tx = if use_liquidswap {
                // Use Liquid Labs DEX aggregator for optimal routing
                info!("Using Liquid Labs DEX aggregator for token0->token1 swap");

                // Get the Liquid Labs router address from config
                let liquidswap_router = Address::from_str(&params.config.hyperevm.liqd_multi_hop_router_address)?;

                // Perform swap using Liquid Labs
                let (tx_hash, output_amount) = swap_with_liquidswap(
                    provider.clone(),
                    params.token0,    // token_in (WHYPE)
                    params.token1,    // token_out (USDT)
                    safe_swap_amount, // Use safe amount
                    params.wallet_address,
                    liquidswap_router,
                    params.config.gas.slippage_tolerance_percent,
                    params.vault_config.use_max_token_approval,
                    decimals0, // token_in decimals
                    decimals1, // token_out decimals
                    &params.config.hyperevm.liquidswap_api_endpoint,
                )
                .await?;

                info!("Liquid Labs swap completed, output amount: {}", output_amount);

                // Convert tx_hash string to proper format for consistency
                tx_hash.parse::<alloy::primitives::TxHash>()?
            } else if is_hyperswap {
                info!("Using HyperSwap router (7 params, no deadline)");
                let hyperswap_router = IHyperSwapRouter::new(params.router_address, provider.clone());
                let swap_params = (
                    params.token0,                                      // tokenIn (WHYPE)
                    params.token1,                                      // tokenOut (USDT)
                    alloy::primitives::Uint::<24, 1>::from(params.fee), // fee
                    params.wallet_address,                              // recipient
                    safe_swap_amount,                                   // amountIn (use safe amount)
                    amount_out_minimum,                                 // amountOutMinimum (5% slippage protection)
                    alloy::primitives::Uint::<160, 3>::ZERO,            // sqrtPriceLimitX96
                );
                let swap_call = hyperswap_router.exactInputSingle(swap_params.into());

                // Log calldata for debugging
                let calldata = swap_call.calldata();
                info!("📋 Router calldata: {}", alloy::primitives::hex::encode(calldata));

                // Estimate gas for swap with better error handling
                let swap_gas_estimate = match swap_call.estimate_gas().await {
                    Ok(gas) => gas,
                    Err(e) => {
                        warn!("Gas estimation failed for token0->token1 swap: {}", e);
                        warn!("This usually indicates: invalid token pair, insufficient pool liquidity, or excessive price impact");
                        warn!("Router address: {}", params.router_address);
                        warn!("Token balance check - Token0 balance: {}, Swap amount: {}", params.balance0, swap_amount_raw);
                        warn!("Pool details - tokenIn: {}, tokenOut: {}, fee: {}", params.token0, params.token1, params.fee);
                        warn!("Expected output: {} raw units ({:.6} USDT tokens)", expected_output, expected_usdt_tokens);
                        return Err(eyre::eyre!("Token0->Token1 swap gas estimation failed: {}", e));
                    }
                };
                let swap_gas_with_buffer = ((swap_gas_estimate as f64) * params.config.gas.gas_estimate_multiplier) as u64;
                info!(
                    "Gas estimate for token0->token1 swap: {} (with {}x multiplier: {})",
                    swap_gas_estimate, params.config.gas.gas_estimate_multiplier, swap_gas_with_buffer
                );

                // Execute swap
                let nonce = provider.get_transaction_count(params.wallet_address).await?;
                info!("📤 Sending token0->token1 swap with nonce: {}", nonce);
                sleep(Duration::from_millis(params.config.gas.nonce_delay_ms)).await;

                let pending_tx = swap_call.gas(swap_gas_with_buffer).gas_price(params.adjusted_gas_price).send().await?;
                let tx_hash = pending_tx.tx_hash();
                info!("Token0->Token1 swap, tx sent: {}", tx_hash);
                info!("🔗 {}", format_tx_link(&params.config.global.explorer_base_url, &tx_hash.to_string()));

                let swap_tx = pending_tx.watch().await?;
                info!("[{}] Token0->Token1 swap completed, confirmed: {}", params.vault_config.format_log_info(), swap_tx);
                swap_tx
            } else {
                info!("Using ProjectX router (8 params, with deadline)");
                let projectx_router = IProjectXRouter::new(params.router_address, provider.clone());
                let deadline = U256::from(Utc::now().timestamp() + TRANSACTION_DEADLINE_SECONDS);
                let swap_params = (
                    params.token0,                                      // tokenIn (WHYPE)
                    params.token1,                                      // tokenOut (USDT)
                    alloy::primitives::Uint::<24, 1>::from(params.fee), // fee
                    params.wallet_address,                              // recipient
                    deadline,                                           // deadline (5th parameter for ProjectX)
                    safe_swap_amount,                                   // amountIn (use safe amount)
                    amount_out_minimum,                                 // amountOutMinimum (5% slippage protection)
                    alloy::primitives::Uint::<160, 3>::ZERO,            // sqrtPriceLimitX96
                );
                let swap_call = projectx_router.exactInputSingle(swap_params.into());

                // Log calldata for debugging
                let calldata = swap_call.calldata();
                info!("📋 Router calldata: {}", alloy::primitives::hex::encode(calldata));

                // Estimate gas for swap with better error handling
                let swap_gas_estimate = match swap_call.estimate_gas().await {
                    Ok(gas) => gas,
                    Err(e) => {
                        warn!("Gas estimation failed for token0->token1 swap: {}", e);
                        warn!("This usually indicates: invalid token pair, insufficient pool liquidity, or excessive price impact");
                        warn!("Router address: {}", params.router_address);
                        warn!("Token balance check - Token0 balance: {}, Swap amount: {}", params.balance0, swap_amount_raw);
                        warn!("Pool details - tokenIn: {}, tokenOut: {}, fee: {}", params.token0, params.token1, params.fee);
                        warn!("Expected output: {} raw units ({:.6} USDT tokens)", expected_output, expected_usdt_tokens);
                        return Err(eyre::eyre!("Token0->Token1 swap gas estimation failed: {}", e));
                    }
                };
                let swap_gas_with_buffer = ((swap_gas_estimate as f64) * params.config.gas.gas_estimate_multiplier) as u64;
                info!(
                    "Gas estimate for token0->token1 swap: {} (with {}x multiplier: {})",
                    swap_gas_estimate, params.config.gas.gas_estimate_multiplier, swap_gas_with_buffer
                );

                // Execute swap
                let nonce = provider.get_transaction_count(params.wallet_address).await?;
                info!("📤 Sending token0->token1 swap with nonce: {}", nonce);
                sleep(Duration::from_millis(params.config.gas.nonce_delay_ms)).await;

                let pending_tx = swap_call.gas(swap_gas_with_buffer).gas_price(params.adjusted_gas_price).send().await?;
                let tx_hash = pending_tx.tx_hash();
                info!("Token0->Token1 swap, tx sent: {}", tx_hash);
                info!("🔗 {}", format_tx_link(&params.config.global.explorer_base_url, &tx_hash.to_string()));

                let swap_tx = pending_tx.watch().await?;
                info!("[{}] Token0->Token1 swap completed, confirmed: {}", params.vault_config.format_log_info(), swap_tx);
                swap_tx
            };

            // Update progress
            let swap_method = if use_liquidswap { "liquidlabs_aggregator".to_string() } else { params.dex_name.to_lowercase() };
            current_step = RebalanceStep::Token0Swapped {
                tx: format!("0x{}", swap_tx),
                amount: safe_swap_amount,
                swap_method,
            };
            token0_swap_tx = Some(format!("0x{}", swap_tx));
        } else {
            warn!("Insufficient token0 balance for swap: need {} but have {}", safe_swap_amount, actual_balance0);
        }
    }
    // Token1 to Token0 swap
    else if params.rebalancing_info.swap_amount_token1 > 0.0 {
        info!("Step 5b: Swapping {} Token1 to Token0", params.rebalancing_info.swap_amount_token1);

        // Convert float amount to raw token units (smallest denomination)
        let (decimals0, decimals1) = get_token_decimals_defaults(params.token0, params.token1);
        let swap_amount_raw = U256::from((params.rebalancing_info.swap_amount_token1 * 10_f64.powi(decimals1 as i32)) as u64);

        // Add balance validation with 1% buffer for safety
        let actual_balance1 = token1_contract.balanceOf(params.wallet_address).call().await?;
        let safe_swap_amount = if actual_balance1 > swap_amount_raw {
            swap_amount_raw // Use calculated amount if we have enough
        } else {
            // Use 99% of actual balance to leave buffer for gas
            let buffered_amount = actual_balance1 * U256::from(99) / U256::from(100);
            warn!(
                "Calculated swap amount {} exceeds balance {}, using buffered amount {}",
                swap_amount_raw, actual_balance1, buffered_amount
            );
            buffered_amount
        };

        // Check if we have enough token1 to swap (after buffer)
        info!("Attempting to swap {} raw units of token1 (have {} raw units)", safe_swap_amount, actual_balance1);
        if safe_swap_amount > U256::ZERO && safe_swap_amount <= actual_balance1 {
            // Determine which router to approve based on whether we're using Liquid Labs
            let approval_router = if use_liquidswap {
                // Use Liquid Labs router for approval
                Address::from_str(&params.config.hyperevm.liqd_multi_hop_router_address)?
            } else {
                // Use the DEX-specific router
                params.router_address
            };

            // Approve router to spend token1
            if token1_contract.allowance(params.wallet_address, approval_router).call().await? < safe_swap_amount {
                let approve_call = token1_contract.approve(approval_router, safe_swap_amount);
                let approve_gas_estimate = approve_call.estimate_gas().await?;
                let approve_gas_with_buffer = ((approve_gas_estimate as f64) * params.config.gas.gas_estimate_multiplier) as u64;
                info!(
                    "Gas estimate for token1 approve: {} (with {}x multiplier: {})",
                    approve_gas_estimate, params.config.gas.gas_estimate_multiplier, approve_gas_with_buffer
                );

                let nonce = provider.get_transaction_count(params.wallet_address).await?;
                info!("📤 Sending token1 swap approval with nonce: {}", nonce);
                sleep(Duration::from_millis(params.config.gas.nonce_delay_ms)).await;

                let pending_tx = token1_contract
                    .approve(approval_router, safe_swap_amount)
                    .gas(approve_gas_with_buffer)
                    .gas_price(params.adjusted_gas_price)
                    .send()
                    .await?;
                let tx_hash = pending_tx.tx_hash();
                info!("Token1 approval for swap, tx sent: {}", tx_hash);
                info!("🔗 {}", format_tx_link(&params.config.global.explorer_base_url, &tx_hash.to_string()));

                let approve_tx = pending_tx.watch().await?;
                info!("Token1 approved for swap, confirmed: {}", approve_tx);
            }

            // Calculate minimum output amount using actual pool price (95% slippage protection)
            let current_price = params.rebalancing_info.current_price; // USDT per WHYPE from pool

            // Convert swap amount from raw USDT units to human readable (use safe amount)
            let swap_amount_tokens = safe_swap_amount.to_string().parse::<f64>().unwrap_or(0.0) / 10_f64.powi(decimals1 as i32);

            // Calculate expected WHYPE output in human readable terms
            let expected_whype_tokens = swap_amount_tokens / current_price;

            // Convert back to raw WHYPE units
            let expected_output = U256::from((expected_whype_tokens * 10_f64.powi(decimals0 as i32)) as u64);
            let amount_out_minimum = expected_output * U256::from(95) / U256::from(100); // ! @ PROD 5% slippage 

            info!("Swap parameters debug:");
            info!("  tokenIn (token1): {}", params.token1);
            info!("  tokenOut (token0): {}", params.token0);
            info!("  fee: {}", params.fee);
            info!("  pool price: {:.6} USDT per WHYPE", current_price);
            info!("  amountIn: {} raw units ({:.6} tokens)", safe_swap_amount, swap_amount_tokens);
            info!("  expected output: {:.6} WHYPE tokens", expected_whype_tokens);
            info!("  amountOutMinimum: {} raw units (95% of expected)", amount_out_minimum);

            // Execute swap with DEX-specific router or Liquid Labs aggregator
            let swap_tx = if use_liquidswap {
                // Use Liquid Labs DEX aggregator for optimal routing
                info!("Using Liquid Labs DEX aggregator for token1->token0 swap");

                // Get the Liquid Labs router address from config
                let liquidswap_router = Address::from_str(&params.config.hyperevm.liqd_multi_hop_router_address)?;

                // Perform swap using Liquid Labs
                let (tx_hash, output_amount) = swap_with_liquidswap(
                    provider.clone(),
                    params.token1,    // token_in (USDT)
                    params.token0,    // token_out (WHYPE)
                    safe_swap_amount, // Use safe amount
                    params.wallet_address,
                    liquidswap_router,
                    params.config.gas.slippage_tolerance_percent,
                    params.vault_config.use_max_token_approval,
                    decimals1, // token_in decimals (USDT)
                    decimals0, // token_out decimals (WHYPE)
                    &params.config.hyperevm.liquidswap_api_endpoint,
                )
                .await?;

                info!("Liquid Labs swap completed, output amount: {}", output_amount);

                // Convert tx_hash string to proper format for consistency
                tx_hash.parse::<alloy::primitives::TxHash>()?
            } else if is_hyperswap {
                info!("Using HyperSwap router (7 params, no deadline)");
                let hyperswap_router = IHyperSwapRouter::new(params.router_address, provider.clone());
                let swap_params = (
                    params.token1,                                      // tokenIn (USDT)
                    params.token0,                                      // tokenOut (WHYPE)
                    alloy::primitives::Uint::<24, 1>::from(params.fee), // fee
                    params.wallet_address,                              // recipient
                    safe_swap_amount,                                   // amountIn (use safe amount)
                    amount_out_minimum,                                 // amountOutMinimum (5% slippage protection)
                    alloy::primitives::Uint::<160, 3>::ZERO,            // sqrtPriceLimitX96
                );
                let swap_call = hyperswap_router.exactInputSingle(swap_params.into());

                // Log calldata for debugging
                let calldata = swap_call.calldata();
                info!("📋 Router calldata: {}", alloy::primitives::hex::encode(calldata));

                // Estimate gas for swap with better error handling
                let swap_gas_estimate = match swap_call.estimate_gas().await {
                    Ok(gas) => gas,
                    Err(e) => {
                        warn!("Gas estimation failed for token1->token0 swap: {}", e);
                        warn!("This usually indicates: invalid token pair, insufficient pool liquidity, or excessive price impact");
                        warn!("Router address: {}", params.router_address);
                        warn!("Token balance check - Token1 balance: {}, Swap amount: {}", params.balance1, swap_amount_raw);
                        warn!("Pool details - tokenIn: {}, tokenOut: {}, fee: {}", params.token1, params.token0, params.fee);
                        warn!("Expected output: {} raw units ({:.6} WHYPE tokens)", expected_output, expected_whype_tokens);
                        return Err(eyre::eyre!("Token1->Token0 swap gas estimation failed: {}", e));
                    }
                };
                let swap_gas_with_buffer = ((swap_gas_estimate as f64) * params.config.gas.gas_estimate_multiplier) as u64;
                info!(
                    "Gas estimate for token1->token0 swap: {} (with {}x multiplier: {})",
                    swap_gas_estimate, params.config.gas.gas_estimate_multiplier, swap_gas_with_buffer
                );

                // Execute swap
                let nonce = provider.get_transaction_count(params.wallet_address).await?;
                info!("📤 Sending token1->token0 swap with nonce: {}", nonce);
                sleep(Duration::from_millis(params.config.gas.nonce_delay_ms)).await;

                let pending_tx = swap_call.gas(swap_gas_with_buffer).gas_price(params.adjusted_gas_price).send().await?;
                let tx_hash = pending_tx.tx_hash();
                info!("Token1->Token0 swap, tx sent: {}", tx_hash);
                info!("🔗 {}", format_tx_link(&params.config.global.explorer_base_url, &tx_hash.to_string()));

                let swap_tx = pending_tx.watch().await?;
                info!("[{}] Token1->Token0 swap completed, confirmed: {}", params.vault_config.format_log_info(), swap_tx);
                swap_tx
            } else {
                info!("Using ProjectX router (8 params, with deadline)");
                let projectx_router = IProjectXRouter::new(params.router_address, provider.clone());
                let deadline = U256::from(Utc::now().timestamp() + TRANSACTION_DEADLINE_SECONDS);
                let swap_params = (
                    params.token1,                                      // tokenIn (USDT)
                    params.token0,                                      // tokenOut (WHYPE)
                    alloy::primitives::Uint::<24, 1>::from(params.fee), // fee
                    params.wallet_address,                              // recipient
                    deadline,                                           // deadline (5th parameter for ProjectX)
                    safe_swap_amount,                                   // amountIn (use safe amount)
                    amount_out_minimum,                                 // amountOutMinimum (5% slippage protection)
                    alloy::primitives::Uint::<160, 3>::ZERO,            // sqrtPriceLimitX96
                );
                let swap_call = projectx_router.exactInputSingle(swap_params.into());

                // Log calldata for debugging
                let calldata = swap_call.calldata();
                info!("📋 Router calldata: {}", alloy::primitives::hex::encode(calldata));

                // Estimate gas for swap with better error handling
                let swap_gas_estimate = match swap_call.estimate_gas().await {
                    Ok(gas) => gas,
                    Err(e) => {
                        warn!("Gas estimation failed for token1->token0 swap: {}", e);
                        warn!("This usually indicates: invalid token pair, insufficient pool liquidity, or excessive price impact");
                        warn!("Router address: {}", params.router_address);
                        warn!("Token balance check - Token1 balance: {}, Swap amount: {}", params.balance1, swap_amount_raw);
                        warn!("Pool details - tokenIn: {}, tokenOut: {}, fee: {}", params.token1, params.token0, params.fee);
                        warn!("Expected output: {} raw units ({:.6} WHYPE tokens)", expected_output, expected_whype_tokens);
                        return Err(eyre::eyre!("Token1->Token0 swap gas estimation failed: {}", e));
                    }
                };
                let swap_gas_with_buffer = ((swap_gas_estimate as f64) * params.config.gas.gas_estimate_multiplier) as u64;
                info!(
                    "Gas estimate for token1->token0 swap: {} (with {}x multiplier: {})",
                    swap_gas_estimate, params.config.gas.gas_estimate_multiplier, swap_gas_with_buffer
                );

                // Execute swap
                let nonce = provider.get_transaction_count(params.wallet_address).await?;
                info!("📤 Sending token1->token0 swap with nonce: {}", nonce);
                sleep(Duration::from_millis(params.config.gas.nonce_delay_ms)).await;

                let pending_tx = swap_call.gas(swap_gas_with_buffer).gas_price(params.adjusted_gas_price).send().await?;
                let tx_hash = pending_tx.tx_hash();
                info!("Token1->Token0 swap, tx sent: {}", tx_hash);
                info!("🔗 {}", format_tx_link(&params.config.global.explorer_base_url, &tx_hash.to_string()));

                let swap_tx = pending_tx.watch().await?;
                info!("[{}] Token1->Token0 swap completed, confirmed: {}", params.vault_config.format_log_info(), swap_tx);
                swap_tx
            };

            // Update progress
            let swap_method = if use_liquidswap { "liquidlabs_aggregator".to_string() } else { params.dex_name.to_lowercase() };
            current_step = RebalanceStep::Token1Swapped {
                tx: format!("0x{}", swap_tx),
                amount: safe_swap_amount,
                swap_method,
            };
            token1_swap_tx = Some(format!("0x{}", swap_tx));
        } else {
            warn!("Insufficient token1 balance for swap: need {} but have {}", safe_swap_amount, actual_balance1);
        }
    }

    // Calculate post-swap balances
    let new_balance0 = token0_contract.balanceOf(params.wallet_address).call().await?;
    let new_balance1 = token1_contract.balanceOf(params.wallet_address).call().await?;
    info!("Post-swap balances - Token0: {}, Token1: {}", new_balance0, new_balance1);

    Ok(SwapResult {
        new_balance0,
        new_balance1,
        token0_swap_tx,
        token1_swap_tx,
        current_step,
    })
}
