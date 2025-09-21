use alloy::{
    primitives::{Address, U256},
    providers::Provider,
};
use eyre::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

// ============================================================================
// API Request/Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct RouteRequest {
    #[serde(rename = "tokenIn")]
    pub token_in: String,
    #[serde(rename = "tokenOut")]
    pub token_out: String,
    #[serde(rename = "amountIn", skip_serializing_if = "Option::is_none")]
    pub amount_in: Option<String>,
    #[serde(rename = "amountOut", skip_serializing_if = "Option::is_none")]
    pub amount_out: Option<String>,
    #[serde(rename = "multiHop", skip_serializing_if = "Option::is_none")]
    pub multi_hop: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slippage: Option<f64>,
    #[serde(rename = "feeBps", skip_serializing_if = "Option::is_none")]
    pub fee_bps: Option<u32>,
    #[serde(rename = "feeRecipient", skip_serializing_if = "Option::is_none")]
    pub fee_recipient: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RouteResponse {
    pub success: bool,
    pub tokens: Option<TokensInfo>,
    #[serde(rename = "amountIn")]
    pub amount_in: String,
    #[serde(rename = "amountOut")]
    pub amount_out: String,
    #[serde(rename = "averagePriceImpact")]
    pub price_impact: Option<String>,
    pub execution: ExecutionData,
}

#[derive(Debug, Deserialize)]
pub struct TokensInfo {
    #[serde(rename = "tokenIn")]
    pub token_in: TokenInfo,
    #[serde(rename = "tokenOut")]
    pub token_out: TokenInfo,
}

#[derive(Debug, Deserialize)]
pub struct TokenInfo {
    pub address: String,
    pub symbol: String,
    pub decimals: u8,
    pub name: String,
    #[serde(rename = "logoURI", skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ExecutionData {
    pub to: String,
    pub calldata: String,
    #[serde(default)]
    pub value: Option<String>,
    pub details: Option<ExecutionDetails>,
}

#[derive(Debug, Deserialize)]
pub struct ExecutionDetails {
    pub path: Vec<String>,
    #[serde(rename = "minAmountOut")]
    pub min_amount_out: String,
}

// ============================================================================
// Core Functions
// ============================================================================

/// Fetch the optimal swap route from LiquidSwap API
///
/// ## Parameters
/// - `token_in`: Input token address
/// - `token_out`: Output token address  
/// - `amount_in`: Amount of input tokens (in wei)
/// - `slippage_percent`: Slippage tolerance in percent (e.g., 1.0 for 1%)
/// - `enable_multi_hop`: Enable multi-hop routing for better rates
/// - `token_in_decimals`: Decimals for the input token
/// - `api_endpoint`: API endpoint URL (e.g., "https://api.liqd.ag/v2")
///
/// ## Returns
/// - `Ok(RouteResponse)`: Optimal route with execution data
/// - `Err(...)`: Error if API call fails
pub async fn get_swap_route(token_in: Address, token_out: Address, amount_in: U256, slippage_percent: f64, enable_multi_hop: bool, token_in_decimals: u8, api_endpoint: &str) -> Result<RouteResponse> {
    let client = reqwest::Client::new();

    // Convert amount from wei to human-readable format based on token decimals
    let amount_in_human = {
        let divisor = U256::from(10).pow(U256::from(token_in_decimals));
        let whole = amount_in / divisor;
        let remainder = amount_in % divisor;

        // Format with up to 6 decimal places
        if remainder == U256::ZERO {
            whole.to_string()
        } else {
            // Calculate decimal part with precision
            let decimal_places = 6;
            let scale = U256::from(10).pow(U256::from(decimal_places));
            let scaled_remainder = remainder * scale / divisor;
            format!("{}.{:0>6}", whole, scaled_remainder).trim_end_matches('0').trim_end_matches('.').to_string()
        }
    };

    let request = RouteRequest {
        token_in: format!("0x{:x}", token_in), // Use lowercase hex format
        token_out: format!("0x{:x}", token_out),
        amount_in: Some(amount_in_human),
        amount_out: None,
        multi_hop: Some(enable_multi_hop),
        slippage: Some(slippage_percent),
        fee_bps: None,
        fee_recipient: None,
    };

    debug!("Fetching route from LiquidSwap:");
    debug!("  Token In: {}", request.token_in);
    debug!("  Token Out: {}", request.token_out);
    debug!("  Amount In: {} (raw: {})", request.amount_in.as_ref().unwrap(), amount_in);
    debug!("  Multi-hop: {:?}", request.multi_hop);
    debug!("  Slippage: {:?}%", request.slippage);

    let url = format!("{}/route", api_endpoint);

    // Log the serialized request for debugging
    if let Ok(_json_str) = serde_json::to_string_pretty(&request) {
        // debug!("Request JSON:\n{}", _json_str);
    }

    let response = client.get(&url).query(&request).send().await?;

    if !response.status().is_success() {
        let error_text = response.text().await?;
        error!("LiquidSwap API error: {}", error_text);
        return Err(eyre::eyre!("LiquidSwap API error: {}", error_text));
    }

    // Get response text for debugging
    let response_text = response.text().await?;
    // debug!("LiquidSwap API raw response: {}", response_text);

    // Try to parse the response
    let route_response: RouteResponse = serde_json::from_str(&response_text).map_err(|e| {
        error!("Failed to parse LiquidSwap response: {}", e);
        error!("Response was: {}", response_text);
        eyre::eyre!("Failed to parse LiquidSwap response: {}", e)
    })?;

    // Log parsed values for debugging
    debug!("Parsed response:");
    debug!("  Amount In: {}", route_response.amount_in);
    debug!("  Amount Out: {}", route_response.amount_out);
    if let Some(ref details) = route_response.execution.details {
        debug!("  Min Amount Out: {}", details.min_amount_out);
    }

    // Parse price impact percentage
    let price_impact_value = route_response.price_impact.as_ref().and_then(|s| s.trim_end_matches('%').parse::<f64>().ok()).unwrap_or(0.0);

    info!("Found route, price impact: {:.4}%", price_impact_value);

    if let Some(details) = &route_response.execution.details {
        debug!("Route path: {:?}", details.path);
        debug!("Min amount out: {}", details.min_amount_out);
    }

    Ok(route_response)
}

/// Check if a token needs approval for the router
///
/// ## Parameters
/// - `provider`: Blockchain provider
/// - `token_address`: Token to check
/// - `owner`: Token owner address
/// - `spender`: Spender address (router)
/// - `amount`: Amount to check approval for
///
/// ## Returns
/// - `Ok(bool)`: true if approval is needed
pub async fn needs_approval<P: Provider>(provider: P, token_address: Address, owner: Address, spender: Address, amount: U256) -> Result<bool> {
    use crate::dex::u3pos::IERC20;

    let token = IERC20::new(token_address, provider);
    let allowance = token.allowance(owner, spender).call().await?;

    Ok(allowance < amount)
}

/// Approve token spending for the router
///
/// ## Parameters
/// - `provider`: Blockchain provider with signer
/// - `token_address`: Token to approve
/// - `spender`: Spender address (router)
/// - `amount`: Amount to approve (use U256::MAX for unlimited)
///
/// ## Returns
/// - `Ok(String)`: Transaction hash
pub async fn approve_token<P: Provider>(provider: P, token_address: Address, spender: Address, amount: U256) -> Result<String> {
    use crate::dex::u3pos::IERC20;

    info!("Approving {} for spending by {}", token_address, spender);

    let token = IERC20::new(token_address, provider);

    // Estimate gas
    let estimated = token.approve(spender, amount).estimate_gas().await?;
    let gas_with_buffer = estimated * 150 / 100; // 50% buffer

    // Send approval transaction
    let tx = token.approve(spender, amount).gas(gas_with_buffer).send().await?;

    let hash = format!("{:?}", tx.tx_hash());
    info!("Approval tx sent: {}", hash);

    // Wait for confirmation using watch() which polls until mined
    let receipt = tx.watch().await?;
    info!("Approval confirmed: {:?}", receipt);

    Ok(hash)
}

/// Execute a swap using LiquidSwap router
///
/// ## Parameters
/// - `provider`: Blockchain provider with signer
/// - `route`: Route response from API
/// - `router_address`: LiquidSwap router contract address
///
/// ## Returns  
/// - `Ok(String)`: Transaction hash of the swap
pub async fn execute_swap<P: Provider>(provider: P, route: &RouteResponse, router_address: Address) -> Result<String> {
    info!("Executing swap via LiquidSwap router at {}", router_address);

    let min_out = route.execution.details.as_ref().map(|d| d.min_amount_out.as_str()).unwrap_or("0");
    info!("Expected output: {} (min: {} wei)", route.amount_out, min_out);

    // Parse the execution data
    let to_address = route.execution.to.parse::<Address>()?;
    let calldata = hex::decode(route.execution.calldata.trim_start_matches("0x"))?;
    let value = route.execution.value.as_ref().and_then(|v| U256::from_str_radix(v, 10).ok()).unwrap_or(U256::ZERO);

    debug!("Execution details:");
    debug!("  To: {}", to_address);
    debug!("  Value: {}", value);
    debug!("  Calldata length: {} bytes", calldata.len());

    // Verify the target address matches our router
    if to_address != router_address {
        error!("Route execution target {} doesn't match router {}", to_address, router_address);
        return Err(eyre::eyre!("Invalid execution target"));
    }

    // Build and send the transaction
    let tx_request = alloy::rpc::types::TransactionRequest::default().to(to_address).input(calldata.into()).value(value);

    // Estimate gas
    let estimated = provider.estimate_gas(tx_request.clone()).await?;
    let gas_with_buffer = estimated * 150 / 100; // 50% buffer

    info!("Sending swap transaction with {} gas", gas_with_buffer);

    // Send transaction
    let pending_tx = provider.send_transaction(tx_request.gas_limit(gas_with_buffer)).await?;

    let hash = format!("{:?}", pending_tx.tx_hash());
    info!("Swap tx sent: {}", hash);

    // Wait for confirmation using watch() which polls until mined
    let receipt = pending_tx.watch().await?;
    info!("Swap confirmed: {:?}", receipt);

    Ok(hash)
}

/// High-level function to perform a token swap with LiquidSwap
///
/// ## Parameters
/// - `provider`: Blockchain provider with signer
/// - `token_in`: Input token address
/// - `token_out`: Output token address
/// - `amount_in`: Amount to swap (in wei)
/// - `wallet_address`: Wallet address performing the swap
/// - `router_address`: LiquidSwap router address
/// - `slippage_percent`: Slippage tolerance
/// - `use_max_approval`: Whether to use MAX approval or exact amount
/// - `token_in_decimals`: Decimals for input token
/// - `token_out_decimals`: Decimals for output token
/// - `api_endpoint`: API endpoint URL (e.g., "https://api.liqd.ag/v2")
///
/// ## Returns
/// - `Ok((String, U256))`: Swap transaction hash and output amount
pub async fn swap_with_liquidswap<P: Provider + Clone>(
    provider: P, token_in: Address, token_out: Address, amount_in: U256, wallet_address: Address, router_address: Address, slippage_percent: f64, use_max_approval: bool, token_in_decimals: u8,
    token_out_decimals: u8, api_endpoint: &str,
) -> Result<(String, U256)> {
    info!("Initiating LiquidSwap aggregator swap");
    info!("Token {} -> {}, amount: {}", token_in, token_out, amount_in);

    // Step 1: Get the optimal route using simple/single-hop routing
    let route = get_swap_route(
        token_in,
        token_out,
        amount_in,
        slippage_percent,
        false, // Use simple/single-hop routing only
        token_in_decimals,
        api_endpoint,
    )
    .await?;

    let price_impact_str = route.price_impact.clone().unwrap_or_else(|| "0%".to_string());
    let min_out = route.execution.details.as_ref().map(|d| d.min_amount_out.clone()).unwrap_or_else(|| "0".to_string());

    info!("Route found with price impact: {}", price_impact_str);
    info!("Min output: {}", min_out);

    // Step 2: Check and handle approval
    info!("Checking approval for {} tokens of {}", amount_in, token_in);
    let needs_approval = needs_approval(provider.clone(), token_in, wallet_address, router_address, amount_in).await?;

    if needs_approval {
        let approval_amount = if use_max_approval { U256::MAX } else { amount_in };
        info!("Approving {} tokens (max: {})", approval_amount, use_max_approval);

        let approval_tx = approve_token(provider.clone(), token_in, router_address, approval_amount).await?;

        info!("Token approved in tx: {}", approval_tx);

        // Wait a bit for approval to propagate
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    } else {
        info!("Token already approved, skipping approval");
    }

    // Check token balance before swap
    use crate::dex::u3pos::IERC20;
    let token_contract = IERC20::new(token_in, provider.clone());
    let balance = token_contract.balanceOf(wallet_address).call().await?;
    info!("Token balance before swap: {}", balance);

    if balance < amount_in {
        return Err(eyre::eyre!("Insufficient token balance: have {} but need {}", balance, amount_in));
    }

    // Step 3: Execute the swap
    let swap_tx = execute_swap(provider, &route, router_address).await?;

    // Parse the output amount - it comes back as human-readable with decimals (e.g., "6022606.190434")
    let output_amount = {
        // Parse as float first, then convert to wei
        let human_amount: f64 = route.amount_out.parse().map_err(|e| eyre::eyre!("Failed to parse output amount '{}': {}", route.amount_out, e))?;

        // Convert to wei based on output token decimals
        let multiplier = 10f64.powi(token_out_decimals as i32);
        let wei_amount = (human_amount * multiplier) as u128;
        U256::from(wei_amount)
    };

    info!("Swap completed successfully!");
    info!("Transaction: {}", swap_tx);
    info!("Output amount: {}", output_amount);

    Ok((swap_tx, output_amount))
}
