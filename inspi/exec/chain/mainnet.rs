/// =============================================================================
/// Mainnet Execution Strategy
/// =============================================================================
///
/// @description: Mainnet execution strategy optimized for Ethereum mainnet with
/// Flashbots support. This strategy provides MEV protection and bundle submission
/// capabilities for secure and efficient transaction execution on Ethereum mainnet.
/// =============================================================================
use async_trait::async_trait;
use std::str::FromStr;

use alloy::{
    network::TransactionBuilder,
    providers::{Provider, ProviderBuilder},
    rpc::types::mev::EthSendBundle,
    signers::local::PrivateKeySigner,
};
use alloy_mev::{BundleSigner, EthMevProviderExt};
use alloy_primitives::B256;

use crate::{
    maker::{exec::ExecStrategyName, tycho::get_alloy_chain},
    types::{
        config::{EnvConfig, MarketMakerConfig},
        maker::{BroadcastData, Trade},
    },
};

use super::super::ExecStrategy;

/// =============================================================================
/// @struct: MainnetExec
/// @description: Mainnet execution strategy implementation
/// @behavior: Optimized for Ethereum mainnet with Flashbots MEV protection
/// =============================================================================
pub struct MainnetExec;

/// =============================================================================
/// @function: new
/// @description: Create a new Mainnet execution strategy instance
/// @return Self: New MainnetExec instance
/// =============================================================================
impl Default for MainnetExec {
    fn default() -> Self {
        Self::new()
    }
}

impl MainnetExec {
    pub fn new() -> Self {
        Self
    }
}

/// =============================================================================
/// TRAIT IMPLEMENTATION: ExecStrategy
/// =============================================================================
/// OVERRIDDEN FUNCTIONS:
/// - name(): Returns "Mainnet_Strategy"
/// - broadcast(): Custom implementation using Flashbots bundles for MEV protection
/// 
/// INHERITED FUNCTIONS (using default implementation):
/// - pre_hook(): Default logging
/// - post_hook(): Default event publishing
/// - execute(): Default orchestration flow
/// - simulate(): Default EVM simulation
/// =============================================================================
#[async_trait]
impl ExecStrategy for MainnetExec {
    /// OVERRIDDEN: Custom strategy name
    fn name(&self) -> String {
        ExecStrategyName::MainnetStrategy.as_str().to_string()
    }

    /// =============================================================================
    /// OVERRIDDEN: Custom broadcast implementation
    /// @description: Replaces default mempool broadcast with Flashbots bundle submission
    /// @param prepared: Vector of trades to broadcast
    /// @param mmc: Market maker configuration
    /// @param env: Environment configuration
    /// @return Result<Vec<BroadcastData>, String>: Broadcast results or error
    /// @behavior: Submits transactions as bundles to Flashbots for MEV protection
    /// @differs_from_default: Uses private mempool via Flashbots instead of public mempool
    /// =============================================================================
    async fn broadcast(&self, prepared: Vec<Trade>, mmc: MarketMakerConfig, env: EnvConfig) -> Result<Vec<BroadcastData>, String> {
        tracing::info!("{}: broadcasting {} transactions on Mainnet via bundle", self.name(), prepared.len());

        let ac = get_alloy_chain(mmc.network_name.as_str().to_string()).expect("Failed to get alloy chain");
        let rpc = mmc.rpc_url.parse::<url::Url>().unwrap().clone();
        let pk = env.wallet_private_key.clone();
        let wallet = PrivateKeySigner::from_bytes(&B256::from_str(&pk).expect("Failed to convert swapper pk to B256")).expect("Failed to private key signer");
        let signer = alloy::network::EthereumWallet::from(wallet.clone());
        let provider = ProviderBuilder::new().with_chain(ac).wallet(signer.clone()).on_http(rpc.clone());

        let buildernet = "https://direct-us.buildernet.org:443".to_string();
        let bsigner = PrivateKeySigner::random();
        let endpoints = provider
            .endpoints_builder()
            .authenticated_endpoint(buildernet.parse::<url::Url>().unwrap(), BundleSigner::flashbots(bsigner.clone()))
            .titan(BundleSigner::flashbots(bsigner.clone()))
            .beaverbuild()
            .flashbots(BundleSigner::flashbots(bsigner.clone()))
            .rsync()
            .build();

        let mut results = Vec::new();

        if env.testing {
            tracing::info!("🧪 Skipping broadcast ! Testing mode enabled");
            return Ok(results);
        }

        for trade in prepared.iter() {
            let bnum = provider.get_block_number().await.expect("Failed to get block number");
            let target_block = bnum + mmc.inclusion_block_delay;
            tracing::info!(
                "{}: Current block: {}, target inclusion block: {} (inclusion_block_delay: {})",
                self.name(),
                bnum,
                target_block,
                mmc.inclusion_block_delay
            );

            let mut bd = BroadcastData::default();
            let time = std::time::SystemTime::now();

            let now = std::time::SystemTime::now();
            let broadcasted_at_ms = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
            bd.broadcasted_at_ms = broadcasted_at_ms;

            match trade.swap.clone().build(&signer).await {
                Ok(tx) => {
                    let hash = tx.tx_hash().to_string();
                    tracing::info!("{}: Expected hash: {:?}", self.name(), hash);
                    bd.hash = hash;
                }
                Err(e) => {
                    tracing::error!("{}: Failed to build transaction: {:?}", self.name(), e);
                }
            }

            let mut txs = vec![];
            match provider.encode_request(trade.swap.clone()).await {
                Ok(swap) => {
                    if let Some(approval) = &trade.approve {
                        match provider.encode_request(approval.clone()).await {
                            Ok(approval) => {
                                txs.push(approval);
                            }
                            Err(e) => {
                                tracing::error!("{}: Failed to encode approval: {:?}", self.name(), e);
                            }
                        }
                    }
                    txs.push(swap);
                }
                Err(e) => {
                    let msg = format!("Failed to encode swap: {:?}", e);
                    tracing::error!("{}: {}", self.name(), msg.clone());
                    bd.broadcast_error = Some(msg.clone());
                    return Err(msg.clone());
                }
            }
            let responses = provider
                .send_eth_bundle(
                    EthSendBundle {
                        txs,
                        block_number: target_block,
                        min_timestamp: None,
                        max_timestamp: None,
                        reverting_tx_hashes: vec![],
                        replacement_uuid: None,
                    },
                    &endpoints,
                )
                .await;

            tracing::info!("Bundle sent successfully. Got {} responses", responses.len());
            for response in responses.iter() {
                let took = time.elapsed().unwrap_or_default().as_millis();
                bd.broadcasted_took_ms = took;
                match response {
                    Ok(response) => {
                        tracing::info!("    =>  Bundle sent successfully ({})", response.bundle_hash);
                    }
                    Err(e) => {
                        tracing::warn!("    =>  Failed to send bundle: {:?}", e);
                    }
                }
            }
            results.push(bd);
        }

        Ok(results)
    }
}
