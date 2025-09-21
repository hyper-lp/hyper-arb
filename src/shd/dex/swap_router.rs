use alloy::{
    network::EthereumWallet,
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use chrono::Utc;
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// ===== ROUTER INTERFACES =====

// Uniswap V3 Router Interface
alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    ISwapRouter,
    r#"[
        {
            "inputs": [
                {
                    "components": [
                        {"internalType": "address", "name": "tokenIn", "type": "address"},
                        {"internalType": "address", "name": "tokenOut", "type": "address"},
                        {"internalType": "uint24", "name": "fee", "type": "uint24"},
                        {"internalType": "address", "name": "recipient", "type": "address"},
                        {"internalType": "uint256", "name": "deadline", "type": "uint256"},
                        {"internalType": "uint256", "name": "amountIn", "type": "uint256"},
                        {"internalType": "uint256", "name": "amountOutMinimum", "type": "uint256"},
                        {"internalType": "uint160", "name": "sqrtPriceLimitX96", "type": "uint160"}
                    ],
                    "internalType": "struct ISwapRouter.ExactInputSingleParams",
                    "name": "params",
                    "type": "tuple"
                }
            ],
            "name": "exactInputSingle",
            "outputs": [{"internalType": "uint256", "name": "amountOut", "type": "uint256"}],
            "stateMutability": "payable",
            "type": "function"
        },
        {
            "inputs": [
                {
                    "components": [
                        {"internalType": "bytes", "name": "path", "type": "bytes"},
                        {"internalType": "address", "name": "recipient", "type": "address"},
                        {"internalType": "uint256", "name": "deadline", "type": "uint256"},
                        {"internalType": "uint256", "name": "amountIn", "type": "uint256"},
                        {"internalType": "uint256", "name": "amountOutMinimum", "type": "uint256"}
                    ],
                    "internalType": "struct ISwapRouter.ExactInputParams",
                    "name": "params",
                    "type": "tuple"
                }
            ],
            "name": "exactInput",
            "outputs": [{"internalType": "uint256", "name": "amountOut", "type": "uint256"}],
            "stateMutability": "payable",
            "type": "function"
        }
    ]"#
);

// ERC20 Interface for approvals
alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IERC20,
    r#"[
        {
            "inputs": [
                {"internalType": "address", "name": "spender", "type": "address"},
                {"internalType": "uint256", "name": "amount", "type": "uint256"}
            ],
            "name": "approve",
            "outputs": [{"internalType": "bool", "name": "", "type": "bool"}],
            "stateMutability": "nonpayable",
            "type": "function"
        },
        {
            "inputs": [
                {"internalType": "address", "name": "owner", "type": "address"},
                {"internalType": "address", "name": "spender", "type": "address"}
            ],
            "name": "allowance",
            "outputs": [{"internalType": "uint256", "name": "", "type": "uint256"}],
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

// Quoter Interface for price quotes
alloy::sol!(
    #[allow(missing_docs)]
    #[sol(rpc)]
    IQuoter,
    r#"[
        {
            "inputs": [
                {"internalType": "address", "name": "tokenIn", "type": "address"},
                {"internalType": "address", "name": "tokenOut", "type": "address"},
                {"internalType": "uint24", "name": "fee", "type": "uint24"},
                {"internalType": "uint256", "name": "amountIn", "type": "uint256"},
                {"internalType": "uint160", "name": "sqrtPriceLimitX96", "type": "uint160"}
            ],
            "name": "quoteExactInputSingle",
            "outputs": [{"internalType": "uint256", "name": "amountOut", "type": "uint256"}],
            "stateMutability": "nonpayable",
            "type": "function"
        },
        {
            "inputs": [
                {"internalType": "bytes", "name": "path", "type": "bytes"},
                {"internalType": "uint256", "name": "amountIn", "type": "uint256"}
            ],
            "name": "quoteExactInput",
            "outputs": [{"internalType": "uint256", "name": "amountOut", "type": "uint256"}],
            "stateMutability": "nonpayable",
            "type": "function"
        }
    ]"#
);

// ===== SWAP STRUCTURES =====

/// Single swap parameters
#[derive(Debug, Clone)]
pub struct SwapParams {
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub amount_out_minimum: U256,
    pub fee: u32,
    pub recipient: Address,
    pub slippage_percent: f64,
}

/// Multi-hop swap path
#[derive(Debug, Clone)]
pub struct SwapPath {
    pub tokens: Vec<Address>,
    pub fees: Vec<u32>,
}

/// Swap result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapResult {
    pub tx_hash: String,
    pub amount_in: U256,
    pub amount_out: U256,
    pub gas_used: u128,
    pub timestamp: i64,
}

/// Quote result from quoter
#[derive(Debug, Clone)]
pub struct QuoteResult {
    pub amount_out: U256,
    pub price_impact: f64,
}

// ===== ROUTER MANAGER =====

pub struct SwapRouter {
    pub provider: Box<dyn std::any::Any>,
    pub router_address: Address,
    pub quoter_address: Address,
    pub wallet_address: Address,
}

impl SwapRouter {
    /// Create new swap router instance
    pub fn new<P: Provider + Clone + 'static>(
        provider: P,
        router_address: Address,
        quoter_address: Address,
        wallet_address: Address,
    ) -> Self {
        Self {
            provider: Box::new(provider),
            router_address,
            quoter_address,
            wallet_address,
        }
    }

    /// Get quote for exact input single hop
    pub async fn quote_exact_input_single<P: Provider + Clone>(
        &self,
        provider: P,
        token_in: Address,
        token_out: Address,
        fee: u32,
        amount_in: U256,
    ) -> Result<QuoteResult> {
        let quoter = IQuoter::new(self.quoter_address, provider);
        
        let amount_out = quoter
            .quoteExactInputSingle(
                token_in,
                token_out,
                fee,
                amount_in,
                U256::ZERO, // No price limit
            )
            .call()
            .await?._0;
        
        // Calculate simple price impact (would need pool data for accurate calculation)
        let price_impact = 0.0; // Placeholder
        
        Ok(QuoteResult {
            amount_out,
            price_impact,
        })
    }

    /// Execute exact input single hop swap
    pub async fn swap_exact_input_single<P: Provider + Clone>(
        &self,
        provider: P,
        params: SwapParams,
    ) -> Result<SwapResult> {
        let router = ISwapRouter::new(self.router_address, provider.clone());
        
        // Set deadline to 5 minutes from now
        let deadline = U256::from((Utc::now().timestamp() + 300) as u64);
        
        // Check and approve token if needed
        self.ensure_approval(provider.clone(), params.token_in, params.amount_in).await?;
        
        // Build swap params
        let swap_params = ISwapRouter::ExactInputSingleParams {
            tokenIn: params.token_in,
            tokenOut: params.token_out,
            fee: params.fee,
            recipient: params.recipient,
            deadline,
            amountIn: params.amount_in,
            amountOutMinimum: params.amount_out_minimum,
            sqrtPriceLimitX96: U256::ZERO, // No price limit
        };
        
        // Execute swap
        let tx = router.exactInputSingle(swap_params).send().await?;
        let tx_hash = format!("0x{}", hex::encode(tx.tx_hash()));
        
        // Wait for receipt
        let receipt = tx.get_receipt().await?;
        
        Ok(SwapResult {
            tx_hash,
            amount_in: params.amount_in,
            amount_out: params.amount_out_minimum, // Actual amount would be in logs
            gas_used: receipt.gas_used,
            timestamp: Utc::now().timestamp(),
        })
    }

    /// Ensure token approval for router
    async fn ensure_approval<P: Provider + Clone>(
        &self,
        provider: P,
        token: Address,
        amount: U256,
    ) -> Result<()> {
        let token_contract = IERC20::new(token, provider.clone());
        
        // Check current allowance
        let allowance = token_contract
            .allowance(self.wallet_address, self.router_address)
            .call()
            .await?._0;
        
        // Approve if needed
        if allowance < amount {
            tracing::info!("Approving {} for router", token);
            let approve_tx = token_contract
                .approve(self.router_address, U256::MAX)
                .send()
                .await?;
            
            let _receipt = approve_tx.get_receipt().await?;
            tracing::info!("Approval successful");
        }
        
        Ok(())
    }
}

// ===== UTILITY FUNCTIONS =====

/// Calculate minimum output with slippage
pub fn calculate_minimum_out(expected_out: U256, slippage_percent: f64) -> U256 {
    let slippage_factor = 1.0 - (slippage_percent / 100.0);
    let min_out = expected_out.to::<f64>() * slippage_factor;
    U256::from(min_out as u128)
}

/// Encode swap path for multi-hop swaps
pub fn encode_path(path: &SwapPath) -> Vec<u8> {
    let mut encoded = Vec::new();
    
    for i in 0..path.tokens.len() {
        // Add token address (20 bytes)
        encoded.extend_from_slice(&path.tokens[i].to_vec());
        
        // Add fee if not last token (3 bytes)
        if i < path.fees.len() {
            let fee_bytes = path.fees[i].to_be_bytes();
            encoded.extend_from_slice(&fee_bytes[1..]); // Take last 3 bytes
        }
    }
    
    encoded
}

/// Build provider with wallet for transactions
pub async fn build_provider_with_wallet(
    rpc_url: &str,
    private_key: &str,
) -> Result<impl Provider + Clone> {
    let signer = PrivateKeySigner::from_str(private_key)?;
    let wallet = EthereumWallet::from(signer);
    
    let provider = ProviderBuilder::new()
        .wallet(wallet)
        .connect(rpc_url)
        .await?;
    
    Ok(provider)
}

/// Flash swap parameters for arbitrage
#[derive(Debug, Clone)]
pub struct FlashSwapParams {
    pub pool_address: Address,
    pub token0: Address,
    pub token1: Address,
    pub amount0: U256,
    pub amount1: U256,
    pub callback_data: Vec<u8>,
}

/// Arbitrage execution parameters
#[derive(Debug, Clone)]
pub struct ArbitrageParams {
    pub buy_pool: Address,
    pub sell_pool: Address,
    pub token_in: Address,
    pub token_out: Address,
    pub amount: U256,
    pub expected_profit: U256,
}

/// Execute arbitrage between two pools
pub async fn execute_arbitrage<P: Provider + Clone>(
    provider: P,
    router_address: Address,
    params: ArbitrageParams,
) -> Result<SwapResult> {
    // This is a simplified version - actual implementation would need:
    // 1. Flash loan or initial capital
    // 2. Buy from cheaper pool
    // 3. Sell to expensive pool
    // 4. Return flash loan if used
    
    tracing::info!(
        "Executing arbitrage: {} -> {} with amount {}",
        params.buy_pool,
        params.sell_pool,
        params.amount
    );
    
    // Placeholder for actual arbitrage logic
    Ok(SwapResult {
        tx_hash: "0x0".to_string(),
        amount_in: params.amount,
        amount_out: params.amount + params.expected_profit,
        gas_used: 0,
        timestamp: Utc::now().timestamp(),
    })
}

/// Get optimal swap route using pathfinding
pub async fn find_optimal_route<P: Provider + Clone>(
    provider: P,
    quoter_address: Address,
    token_in: Address,
    token_out: Address,
    amount_in: U256,
    possible_pools: Vec<(Address, u32)>, // (pool_address, fee)
) -> Result<(SwapPath, U256)> {
    let mut best_path = SwapPath {
        tokens: vec![token_in, token_out],
        fees: vec![3000], // Default 0.3% fee
    };
    let mut best_output = U256::ZERO;
    
    // Try direct routes through each pool
    for (pool, fee) in possible_pools {
        let quoter = IQuoter::new(quoter_address, provider.clone());
        
        match quoter
            .quoteExactInputSingle(
                token_in,
                token_out,
                fee,
                amount_in,
                U256::ZERO,
            )
            .call()
            .await
        {
            Ok(result) => {
                if result._0 > best_output {
                    best_output = result._0;
                    best_path.fees = vec![fee];
                    tracing::info!("Found better route through pool {} with output {}", pool, result._0);
                }
            }
            Err(e) => {
                tracing::debug!("Pool {} quote failed: {}", pool, e);
            }
        }
    }
    
    Ok((best_path, best_output))
}

/// Estimate gas for swap
pub async fn estimate_swap_gas<P: Provider + Clone>(
    provider: P,
    router_address: Address,
    params: &SwapParams,
) -> Result<u128> {
    let router = ISwapRouter::new(router_address, provider);
    
    let deadline = U256::from((Utc::now().timestamp() + 300) as u64);
    
    let swap_params = ISwapRouter::ExactInputSingleParams {
        tokenIn: params.token_in,
        tokenOut: params.token_out,
        fee: params.fee,
        recipient: params.recipient,
        deadline,
        amountIn: params.amount_in,
        amountOutMinimum: params.amount_out_minimum,
        sqrtPriceLimitX96: U256::ZERO,
    };
    
    let gas_estimate = router.exactInputSingle(swap_params).estimate_gas().await?;
    
    Ok(gas_estimate)
}