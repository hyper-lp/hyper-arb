use alloy::{
    network::EthereumWallet,
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionReceipt,
    signers::local::PrivateKeySigner,
};
use chrono::Utc;
use eyre::Result;
use std::str::FromStr;
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};

use crate::{
    dex::u3pos::{IERC20, INonfungiblePositionManager, get_token_decimals_defaults, tick_to_price},
    misc::utils::format_tx_link,
    types::config::BotConfig,
};

/// Transaction deadline in seconds (5 minutes)
const TRANSACTION_DEADLINE_SECONDS: i64 = 300;

/// ERC721 Transfer event signature: Transfer(address indexed from, address indexed to, uint256 indexed tokenId)
const ERC721_TRANSFER_EVENT_SIGNATURE: &str = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

/// IncreaseLiquidity event signature: IncreaseLiquidity(indexed uint256 tokenId, uint128 liquidity, uint256 amount0, uint256 amount1)
const INCREASE_LIQUIDITY_EVENT_SIGNATURE: &str = "0x3067048beee31b25b2f1681f88dac838c8bba36af25bfb2b7cf7473a5847e35f";

/// Extracts token ID from mint receipt by finding ERC721 Transfer from zero address
fn extract_token_id_from_receipt(receipt: &TransactionReceipt, position_manager_address: Address, recipient: Address) -> Result<U256> {
    debug!("Searching for ERC721 Transfer event in transaction with {} logs", receipt.logs().len());
    debug!("Looking for mint to address: {} from position manager: {}", recipient, position_manager_address);
    // Look for Transfer events from the position manager contract
    for log in receipt.logs() {
        // Check if this log is from the position manager contract
        if log.address() != position_manager_address {
            continue;
        }

        // Check if this is a Transfer event by comparing the event signature
        let topics = log.topics();
        if topics.len() != 4 {
            continue; // Transfer event should have exactly 4 topics
        }

        let event_signature = topics[0];
        if format!("{:?}", event_signature) != ERC721_TRANSFER_EVENT_SIGNATURE {
            continue; // Not a Transfer event
        }

        // Extract the from, to, and tokenId from the topics
        let from_topic = topics[1];
        let to_topic = topics[2];
        let token_id_topic = topics[3];

        // Convert topics to addresses/values for comparison
        let from_address = Address::from_word(from_topic);
        let to_address = Address::from_word(to_topic);
        let token_id = U256::from_be_bytes(token_id_topic.into());

        // Check if this is a mint (from zero address) to our recipient
        if from_address == Address::ZERO && to_address == recipient {
            info!("Found NFT mint event: token ID {} minted to {}", token_id, recipient);
            return Ok(token_id);
        }
    }

    warn!("Could not find ERC721 Transfer event for minted NFT");
    warn!(
        "Transaction had {} logs from position manager contract",
        receipt.logs().iter().filter(|log| log.address() == position_manager_address).count()
    );
    Err(eyre::eyre!("Could not find ERC721 Transfer event for minted NFT in transaction receipt"))
}

/// Extracts actual deposited amounts from IncreaseLiquidity event
fn extract_actual_amounts_from_receipt(receipt: &TransactionReceipt, position_manager_address: Address, token_id: U256) -> Result<(U256, U256)> {
    debug!("Searching for IncreaseLiquidity event for token ID: {}", token_id);

    for log in receipt.logs() {
        // Check if this log is from the position manager contract
        if log.address() != position_manager_address {
            continue;
        }

        // Check if this is an IncreaseLiquidity event
        let topics = log.topics();
        if topics.len() != 2 {
            continue; // IncreaseLiquidity has 2 topics (signature + indexed tokenId)
        }

        let event_signature = topics[0];
        if format!("{:?}", event_signature) != INCREASE_LIQUIDITY_EVENT_SIGNATURE {
            continue;
        }

        // Check if this is for our token ID (topic[1] is the indexed tokenId)
        let event_token_id = U256::from_be_bytes(topics[1].into());
        if event_token_id != token_id {
            continue;
        }

        // Parse the data field which contains: liquidity (uint128), amount0 (uint256), amount1 (uint256)
        let data = log.data().data.as_ref();
        if data.len() >= 96 {
            // Skip first 32 bytes (liquidity), get amount0 and amount1
            let amount0 = U256::from_be_bytes::<32>(data[32..64].try_into().unwrap_or_default());
            let amount1 = U256::from_be_bytes::<32>(data[64..96].try_into().unwrap_or_default());

            info!("✅ Found IncreaseLiquidity event - actual amount0: {}, actual amount1: {}", amount0, amount1);
            return Ok((amount0, amount1));
        }
    }

    warn!("⚠️ Could not find IncreaseLiquidity event, using ordered amounts as fallback");
    Err(eyre::eyre!("Could not find IncreaseLiquidity event in transaction receipt"))
}

/// Parameters for minting a new LP position
#[derive(Debug, Clone)]
pub struct MintPositionParams {
    /// Token addresses (will be ordered automatically)
    pub token0: Address,
    pub token1: Address,

    /// Pool fee tier (100=0.01%, 500=0.05%, 3000=0.3%, 10000=1%)
    pub fee: u32,

    /// Target tick range for the position
    pub tick_lower: i32,
    pub tick_upper: i32,

    /// Token amounts to deposit (max amounts, actual may be less based on price)
    pub amount0_desired: U256,
    pub amount1_desired: U256,

    /// Slippage protection (set to 0 for no protection)
    pub amount0_min: U256,
    pub amount1_min: U256,
}

/// Result from minting a new position
#[derive(Debug, Clone)]
pub struct MintPositionResult {
    /// The NFT token ID of the newly minted position
    pub token_id: U256,

    /// Transaction hash of the mint transaction
    pub mint_tx: String,

    /// Optional: Token approval transaction hashes
    pub token0_approval_tx: Option<String>,
    pub token1_approval_tx: Option<String>,

    /// Ordered amounts (what was requested to deposit)
    pub ordered_amount0: U256,
    pub ordered_amount1: U256,

    /// Actual amounts deposited (from IncreaseLiquidity event)
    pub amount0_deposited: U256,
    pub amount1_deposited: U256,
}

/// Mints a new Uniswap V3 LP position with automatic token approvals
///
/// ## Overview
/// This function handles the complete process of minting a new concentrated liquidity position:
/// 1. **Check Balances**: Verify sufficient token balances
/// 2. **Approve Tokens**: Grant position manager permission to spend tokens (if needed)
/// 3. **Align Ticks**: Ensure ticks are valid for the pool's tick spacing
/// 4. **Mint Position**: Create the new LP position NFT
///
/// ## Parameters
/// - `config`: Bot configuration with RPC endpoint settings
/// - `private_key`: Private key for transaction signing
/// - `position_manager_address`: Uniswap V3 NonfungiblePositionManager contract
/// - `params`: Minting parameters including tokens, amounts, and tick range
/// - `infinite_approve`: If true, approve MAX amount; if false, approve exact amounts
///
/// ## Returns
/// - `Ok(MintPositionResult)`: Details about the minted position including token ID
/// - `Err(...)`: Detailed error if any step fails
///
/// ## Example
/// ```rust
/// let params = MintPositionParams {
///     token0: hype_address,
///     token1: usdt_address,
///     fee: 500, // 0.05%
///     tick_lower: -1000,
///     tick_upper: 1000,
///     amount0_desired: U256::from(1000000),
///     amount1_desired: U256::from(1000000),
///     amount0_min: U256::ZERO, // No slippage protection
///     amount1_min: U256::ZERO,
/// };
///
/// let result = mint_new_position(
///     &config,
///     &private_key,
///     position_manager_address,
///     params,
///     true, // Use infinite approval
/// ).await?;
/// ```
pub async fn mint_new_position(config: &BotConfig, private_key: &str, position_manager_address: Address, params: MintPositionParams, infinite_approve: bool) -> Result<MintPositionResult> {
    // Setup blockchain connection and wallet
    let signer = PrivateKeySigner::from_str(private_key)?;
    let wallet_address = signer.address();
    let wallet = EthereumWallet::from(signer);
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(config.global.rpc_endpoint.parse()?);

    info!("🔧 Starting mint new position");
    debug!("Wallet address: {}", wallet_address);

    // Get current gas price and apply multiplier
    let current_gas_price = provider.get_gas_price().await?;
    let adjusted_gas_price = (current_gas_price as f64 * config.gas.gas_price_multiplier) as u128;
    info!(
        "⛽ Gas price for mint: base {} wei, adjusted {} wei ({}x multiplier)",
        current_gas_price, adjusted_gas_price, config.gas.gas_price_multiplier
    );

    // Initialize contracts
    let position_manager = INonfungiblePositionManager::new(position_manager_address, provider.clone());
    let token0_contract = IERC20::new(params.token0, provider.clone());
    let token1_contract = IERC20::new(params.token1, provider.clone());

    // =============================================================================
    // STEP 1: CHECK TOKEN BALANCES
    // =============================================================================
    info!("Step 1: Checking token balances");

    let balance0 = token0_contract.balanceOf(wallet_address).call().await?;
    let balance1 = token1_contract.balanceOf(wallet_address).call().await?;
    info!("Current balances - Token0: {}, Token1: {}", balance0, balance1);

    // Verify sufficient balances
    if balance0 < params.amount0_desired {
        return Err(eyre::eyre!("Insufficient token0 balance: have {} but need {}", balance0, params.amount0_desired));
    }
    if balance1 < params.amount1_desired {
        return Err(eyre::eyre!("Insufficient token1 balance: have {} but need {}", balance1, params.amount1_desired));
    }

    // =============================================================================
    // STEP 2: APPROVE TOKENS FOR POSITION MANAGER
    // =============================================================================
    info!("Step 2: Approving tokens for position manager");

    // Check current allowances
    let allowance0 = token0_contract.allowance(wallet_address, position_manager_address).call().await?;

    let allowance1 = token1_contract.allowance(wallet_address, position_manager_address).call().await?;

    // Track approval transactions
    let mut token0_approval_tx = None;
    let mut token1_approval_tx = None;

    // Determine approval amounts based on infinite_approve flag
    let approval_amount0 = if infinite_approve { U256::MAX } else { params.amount0_desired };

    let approval_amount1 = if infinite_approve { U256::MAX } else { params.amount1_desired };

    // Approve Token0 if needed
    if allowance0 < params.amount0_desired {
        info!("Approving token0: current allowance {} < required {}", allowance0, params.amount0_desired);

        let approve0_call = token0_contract.approve(position_manager_address, approval_amount0);
        let approve0_gas_estimate = approve0_call.estimate_gas().await?;
        let approve0_gas_with_buffer = ((approve0_gas_estimate as f64) * config.gas.gas_estimate_multiplier) as u64;
        info!(
            "Gas estimate for token0 approval: {} (with {}x multiplier: {})",
            approve0_gas_estimate, config.gas.gas_estimate_multiplier, approve0_gas_with_buffer
        );

        let nonce = provider.get_transaction_count(wallet_address).await?;
        info!("📤 Sending token0 approval with nonce: {}", nonce);
        sleep(Duration::from_millis(config.gas.nonce_delay_ms)).await;

        let pending_tx = token0_contract
            .approve(position_manager_address, approval_amount0)
            .gas(approve0_gas_with_buffer)
            .gas_price(adjusted_gas_price)
            .send()
            .await?;
        let tx_hash = pending_tx.tx_hash();
        info!("Token0 approval, tx sent: {}", tx_hash);
        info!("🔗 {}", format_tx_link(&config.global.explorer_base_url, &tx_hash.to_string()));

        let approve0_tx = pending_tx.watch().await?;
        info!("Token0 approved, confirmed: {}", approve0_tx);
        token0_approval_tx = Some(format!("0x{}", approve0_tx));
    } else {
        info!("Token0 already has sufficient allowance: {} >= {}", allowance0, params.amount0_desired);
    }

    // Approve Token1 if needed
    if allowance1 < params.amount1_desired {
        info!("Approving token1: current allowance {} < required {}", allowance1, params.amount1_desired);

        let approve1_call = token1_contract.approve(position_manager_address, approval_amount1);
        let approve1_gas_estimate = approve1_call.estimate_gas().await?;
        let approve1_gas_with_buffer = ((approve1_gas_estimate as f64) * config.gas.gas_estimate_multiplier) as u64;
        info!(
            "Gas estimate for token1 approval: {} (with {}x multiplier: {})",
            approve1_gas_estimate, config.gas.gas_estimate_multiplier, approve1_gas_with_buffer
        );

        let nonce = provider.get_transaction_count(wallet_address).await?;
        info!("📤 Sending token1 approval with nonce: {}", nonce);
        sleep(Duration::from_millis(config.gas.nonce_delay_ms)).await;

        let pending_tx = token1_contract
            .approve(position_manager_address, approval_amount1)
            .gas(approve1_gas_with_buffer)
            .gas_price(adjusted_gas_price)
            .send()
            .await?;
        let tx_hash = pending_tx.tx_hash();
        info!("Token1 approval, tx sent: {}", tx_hash);
        info!("🔗 {}", format_tx_link(&config.global.explorer_base_url, &tx_hash.to_string()));

        let approve1_tx = pending_tx.watch().await?;
        info!("Token1 approved, confirmed: {}", approve1_tx);
        token1_approval_tx = Some(format!("0x{}", approve1_tx));
    } else {
        info!("Token1 already has sufficient allowance: {} >= {}", allowance1, params.amount1_desired);
    }

    // =============================================================================
    // STEP 3: PREPARE AND MINT NEW POSITION
    // =============================================================================
    info!("Step 3: Minting new position with tick range [{}, {}]", params.tick_lower, params.tick_upper);

    // Ensure tokens are properly ordered (token0 < token1 by address)
    let (ordered_token0, ordered_token1, ordered_amount0, ordered_amount1, ordered_min0, ordered_min1) = if params.token0 < params.token1 {
        (params.token0, params.token1, params.amount0_desired, params.amount1_desired, params.amount0_min, params.amount1_min)
    } else {
        (params.token1, params.token0, params.amount1_desired, params.amount0_desired, params.amount1_min, params.amount0_min)
    };

    info!("Token ordering check:");
    info!("  Original: token0={}, token1={}", params.token0, params.token1);
    info!("  Ordered:  token0={}, token1={}", ordered_token0, ordered_token1);
    info!("  Token0 < Token1: {}", ordered_token0 < ordered_token1);

    // Get tick spacing for this fee tier and align ticks
    let tick_spacing = match params.fee {
        100 => 1,     // 0.01% fee
        500 => 10,    // 0.05% fee
        3000 => 60,   // 0.30% fee
        10000 => 200, // 1.00% fee
        _ => {
            warn!("Unknown fee tier {}, using default tick spacing of 10", params.fee);
            10
        }
    };

    // Align ticks to valid multiples of tick spacing
    let aligned_tick_lower = params.tick_lower - (params.tick_lower.rem_euclid(tick_spacing));
    let aligned_tick_upper = if params.tick_upper % tick_spacing == 0 {
        params.tick_upper
    } else {
        params.tick_upper + (tick_spacing - params.tick_upper.rem_euclid(tick_spacing))
    };

    // Show original vs aligned ticks if they differ
    if aligned_tick_lower != params.tick_lower || aligned_tick_upper != params.tick_upper {
        info!("  Original Ticks: [{}, {}]", params.tick_lower, params.tick_upper);
        info!("  Aligned Ticks:  [{}, {}] (spacing: {})", aligned_tick_lower, aligned_tick_upper, tick_spacing);
    } else {
        info!("  Tick Range: [{}, {}] (already aligned to spacing {})", aligned_tick_lower, aligned_tick_upper, tick_spacing);
    }

    // Convert aligned ticks to prices for verification
    let (decimals0, decimals1) = get_token_decimals_defaults(ordered_token0, ordered_token1);
    let aligned_price_lower = tick_to_price(aligned_tick_lower, decimals0, decimals1);
    let aligned_price_upper = tick_to_price(aligned_tick_upper, decimals0, decimals1);
    info!("  Price Range: [{:.8} - {:.8}] (token1/token0)", aligned_price_lower, aligned_price_upper);

    // Create mint parameters
    let mint_params = INonfungiblePositionManager::MintParams {
        token0: ordered_token0,
        token1: ordered_token1,
        fee: alloy::primitives::Uint::<24, 1>::from(params.fee),
        tickLower: alloy::primitives::Signed::<24, 1>::try_from(aligned_tick_lower).unwrap_or_default(),
        tickUpper: alloy::primitives::Signed::<24, 1>::try_from(aligned_tick_upper).unwrap_or_default(),
        amount0Desired: ordered_amount0,
        amount1Desired: ordered_amount1,
        amount0Min: ordered_min0,
        amount1Min: ordered_min1,
        recipient: wallet_address,
        deadline: U256::from(Utc::now().timestamp() + TRANSACTION_DEADLINE_SECONDS),
    };

    info!("Minting new position with:");
    info!("  Token0: {} (amount: {})", ordered_token0, ordered_amount0);
    info!("  Token1: {} (amount: {})", ordered_token1, ordered_amount1);
    info!("  Fee: {}", params.fee);

    // Estimate gas for mint transaction
    let mint_call = position_manager.mint(mint_params.clone());
    let mint_gas_with_buffer = match mint_call.estimate_gas().await {
        Ok(gas_estimate) => {
            let gas_with_buffer = ((gas_estimate as f64) * config.gas.gas_estimate_multiplier) as u64;
            info!("Gas estimate for mint: {} (with {}x multiplier: {})", gas_estimate, config.gas.gas_estimate_multiplier, gas_with_buffer);
            gas_with_buffer
        }
        Err(e) => {
            warn!("Failed to estimate gas for mint: {}", e);
            warn!("This likely means the mint transaction will fail");
            warn!("Common causes:");
            warn!("  - Pool doesn't exist for this token pair and fee tier");
            warn!("  - Invalid tick range for the current pool price");
            warn!("  - Insufficient token balances or approvals");
            return Err(eyre::eyre!("Mint gas estimation failed: {}", e));
        }
    };

    // Execute mint transaction
    let nonce = provider.get_transaction_count(wallet_address).await?;
    info!("📤 Sending mint with nonce: {}", nonce);
    sleep(Duration::from_millis(config.gas.nonce_delay_ms)).await;

    let pending_tx = position_manager.mint(mint_params).gas(mint_gas_with_buffer).gas_price(adjusted_gas_price).send().await?;
    let tx_hash = *pending_tx.tx_hash();
    info!("Minting new position, tx sent: {}", tx_hash);
    info!("🔗 {}", format_tx_link(&config.global.explorer_base_url, &tx_hash.to_string()));

    let mint_result = pending_tx.watch().await?;
    info!("New position minted successfully, tx confirmed: {}", mint_result);

    // Get the transaction receipt to extract the actual token ID
    let receipt = provider
        .get_transaction_receipt(tx_hash)
        .await?
        .ok_or_else(|| eyre::eyre!("Transaction receipt not found for hash: {}", tx_hash))?;

    // Extract the real token ID from the ERC721 Transfer event
    let new_token_id = extract_token_id_from_receipt(&receipt, position_manager_address, wallet_address)?;
    info!("✅ Successfully extracted token ID from mint transaction: {}", new_token_id);

    // Extract the actual deposited amounts from the IncreaseLiquidity event
    let (actual_amount0, actual_amount1) = match extract_actual_amounts_from_receipt(&receipt, position_manager_address, new_token_id) {
        Ok((amt0, amt1)) => {
            info!("📊 Actual amounts deposited - token0: {}, token1: {}", amt0, amt1);
            info!("📊 Ordered amounts were - token0: {}, token1: {}", ordered_amount0, ordered_amount1);
            let unused0 = if ordered_amount0 > amt0 { ordered_amount0 - amt0 } else { U256::ZERO };
            let unused1 = if ordered_amount1 > amt1 { ordered_amount1 - amt1 } else { U256::ZERO };
            if unused0 > U256::ZERO || unused1 > U256::ZERO {
                info!("💰 Unused amounts returned to wallet - token0: {}, token1: {}", unused0, unused1);
            }
            (amt0, amt1)
        }
        Err(e) => {
            warn!("Failed to extract actual amounts from IncreaseLiquidity event: {}", e);
            warn!("Using ordered amounts as fallback (may be inaccurate)");
            (ordered_amount0, ordered_amount1)
        }
    };

    // Return the result with both ordered and actual amounts
    Ok(MintPositionResult {
        token_id: new_token_id,
        mint_tx: format!("0x{}", mint_result),
        token0_approval_tx,
        token1_approval_tx,
        ordered_amount0,
        ordered_amount1,
        amount0_deposited: actual_amount0,
        amount1_deposited: actual_amount1,
    })
}
