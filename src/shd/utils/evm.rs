use std::sync::Arc;

use alloy::{
    eips::eip1559::Eip1559Estimation,
    network::{Ethereum, EthereumWallet},
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::types::TransactionReceipt,
    signers::local::PrivateKeySigner,
};

use crate::{
    sol::IERC20,
    types::{BotConfig, EnvConfig},
};

/// =============================================================================
/// @function: balances
/// @description: Get token balances for a specific owner address across multiple tokens
/// @param provider: Alloy provider instance
/// @param owner: Owner address as string
/// @param tokens: Vector of token contract addresses
/// @return Result<Vec<u128>, String>: Vector of token balances in wei or error
/// =============================================================================
pub async fn balances(provider: String, owner: String, tokens: Vec<String>) -> Result<Vec<u128>, String> {
    let provider = RootProvider::<Ethereum>::new_http(provider.parse().unwrap());
    let mut balances = vec![];
    let client = Arc::new(provider);

    for token in tokens {
        let contract = IERC20::new(token.parse().unwrap(), client.clone());
        match contract.balanceOf(owner.parse().unwrap()).call().await {
            Ok(res) => {
                let balance = res.to_string().parse::<u128>().unwrap_or_default();
                balances.push(balance);
            }
            Err(e) => {
                tracing::error!("Failed to get balance for {}: {:?}", token, e);
                balances.push(0);
            }
        }
    }

    Ok(balances)
}

/// =============================================================================
/// @function: allowance
/// @description: Get the allowance amount for a specific token between owner and spender
/// @param provider: Alloy provider instance
/// @param owner: Token owner address
/// @param spender: Spender address
/// @param token: Token contract address
/// @return Result<u128, String>: Allowance amount in wei or error
/// =============================================================================
pub async fn allowance(rpc: String, owner: String, spender: String, token: String) -> Result<u128, String> {
    let provider = RootProvider::<Ethereum>::new_http(rpc.parse().unwrap());
    let client = Arc::new(provider);
    let contract = IERC20::new(token.parse().unwrap(), client.clone());
    match contract.allowance(owner.parse().unwrap(), spender.parse().unwrap()).call().await {
        Ok(allowance) => Ok(allowance.to_string().parse::<u128>().unwrap_or_default()),
        Err(e) => {
            tracing::error!("Failed to get allowance for {}: {:?}", token, e);
            Err(format!("Failed to get allowance for {}: {:?}", token, e))
        }
    }
}

/// =============================================================================
/// @function: eip1559_fees
/// @description: Estimate EIP-1559 gas fees (max fee and priority fee) for the network
/// @param provider: RPC endpoint URL as string
/// @return Result<Eip1559Estimation, String>: EIP-1559 fee estimation or error
/// =============================================================================
pub async fn eip1559_fees(provider: String) -> Result<Eip1559Estimation, String> {
    let provider = RootProvider::<Ethereum>::new_http(provider.parse().unwrap());
    match provider.estimate_eip1559_fees().await {
        Ok(fees) => Ok(fees),
        Err(e) => {
            tracing::error!("Failed to estimate EIP-1559 fees: {:?}", e);
            Err(format!("Failed to call estimate_eip1559_fees: {:?}", e))
        }
    }
}

/// Approve token spending
pub async fn approve(rpc: &str, signer: &PrivateKeySigner, spender: &str, token: &str, amount: u128) -> Option<TransactionReceipt> {
    let wallet = EthereumWallet::from(signer.clone());
    let provider = ProviderBuilder::new().wallet(wallet).connect_http(rpc.parse().unwrap());

    let token_addr: Address = token.parse().unwrap();
    let spender_addr: Address = spender.parse().unwrap();
    let contract = IERC20::new(token_addr, Arc::new(provider.clone()));

    match contract.approve(spender_addr, U256::from(amount)).send().await {
        Ok(pending) => match pending.get_receipt().await {
            Ok(receipt) => {
                tracing::info!("Approval tx: 0x{:x} | https://hyperevmscan.io/tx/0x{:x}", receipt.transaction_hash, receipt.transaction_hash);
                Some(receipt)
            }
            Err(e) => {
                tracing::error!("Failed to get receipt: {:?}", e);
                None
            }
        },
        Err(e) => {
            tracing::error!("Failed to approve: {:?}", e);
            None
        }
    }
}

/// Get token decimals and balances for two tokens
pub async fn get_token_info_and_balances(
    rpc: &str,
    owner: &str,
    base_token: &str,
    quote_token: &str,
) -> Result<(u8, u8, u128, u128), String> {
    let provider = RootProvider::<Ethereum>::new_http(rpc.parse().unwrap());
    let client = Arc::new(provider);
    
    // Parse addresses
    let base_addr: Address = base_token.parse().map_err(|e| format!("Invalid base token address: {}", e))?;
    let quote_addr: Address = quote_token.parse().map_err(|e| format!("Invalid quote token address: {}", e))?;
    let owner_addr: Address = owner.parse().map_err(|e| format!("Invalid owner address: {}", e))?;
    
    // Get base token info
    let base_contract = IERC20::new(base_addr, client.clone());
    let base_decimals = base_contract.decimals().call().await
        .map_err(|e| format!("Failed to get base decimals: {:?}", e))?;
    let base_balance = base_contract.balanceOf(owner_addr).call().await
        .map_err(|e| format!("Failed to get base balance: {:?}", e))?;
    
    // Get quote token info
    let quote_contract = IERC20::new(quote_addr, client.clone());
    let quote_decimals = quote_contract.decimals().call().await
        .map_err(|e| format!("Failed to get quote decimals: {:?}", e))?;
    let quote_balance = quote_contract.balanceOf(owner_addr).call().await
        .map_err(|e| format!("Failed to get quote balance: {:?}", e))?;
    
    Ok((
        base_decimals,
        quote_decimals,
        base_balance.to::<u128>(),
        quote_balance.to::<u128>(),
    ))
}

pub async fn init_allowance(config: &BotConfig, env: &EnvConfig) {
    let target_allowance = u128::MAX / 2;
    let approve_amount = u128::MAX;

    // Get DEX routers
    let hyperswap = config.get_dex("hyperswap");
    let projectx = config.get_dex("projectx");

    for target in &config.targets {
        if !target.infinite_approval {
            tracing::info!("Target {} has infinite_approval disabled, skipping", target.vault_name);
            continue;
        }

        tracing::info!("Checking allowances for target: {}", target.vault_name);

        // Get signer for this target
        let signer = match env.get_signer_for_address(&target.address) {
            Some(s) => s,
            None => {
                tracing::error!("No signer for {}", target.address);
                continue;
            }
        };

        // Check both DEXs
        let routers = vec![("Hyperswap", hyperswap.map(|d| &d.router)), ("ProjectX", projectx.map(|d| &d.router))];

        for (dex_name, router) in routers {
            if let Some(router_addr) = router {
                // Check base token allowance
                let base_allowance = allowance(config.global.rpc_endpoint.clone(), target.address.clone(), router_addr.clone(), target.base_token_address.clone())
                    .await
                    .unwrap_or(0);

                if base_allowance < target_allowance {
                    tracing::info!("{} {} allowance insufficient, approving...", dex_name, target.base_token);
                    approve(&config.global.rpc_endpoint, &signer, router_addr, &target.base_token_address, approve_amount).await;
                }

                // Check quote token allowance
                let quote_allowance = allowance(config.global.rpc_endpoint.clone(), target.address.clone(), router_addr.clone(), target.quote_token_address.clone())
                    .await
                    .unwrap_or(0);

                if quote_allowance < target_allowance {
                    tracing::info!("{} {} allowance insufficient, approving...", dex_name, target.quote_token);
                    approve(&config.global.rpc_endpoint, &signer, router_addr, &target.quote_token_address, approve_amount).await;
                }
            }
        }
    }
}
