/// =============================================================================
/// Execution Strategy Module
/// =============================================================================
///
/// @description: Execution strategies for different blockchain networks. This module
/// provides network-specific execution logic including simulation, broadcasting,
/// and transaction management for Ethereum, Base, and Unichain networks.
/// =============================================================================
use async_trait::async_trait;
use std::result::Result;
use std::str::FromStr;

use alloy::{
    providers::{Provider, ProviderBuilder},
    rpc::types::simulate::{SimBlock, SimulatePayload},
    signers::local::PrivateKeySigner,
};
use alloy_primitives::B256;

use crate::{
    maker::tycho::get_alloy_chain,
    types::{
        config::{EnvConfig, MarketMakerConfig, NetworkName},
        maker::{BroadcastData, SimulatedData, Trade, TradeStatus},
        moni::NewTradeMessage,
    },
};

pub mod chain;

/// =============================================================================
/// @enum: ExecStrategyName
/// @description: Enumeration of available execution strategy names
/// @variants:
/// - MainnetStrategy: Ethereum mainnet execution strategy
/// - BaseStrategy: Base L2 execution strategy
/// - UnichainStrategy: Unichain execution strategy
/// =============================================================================
#[derive(Debug, Clone, PartialEq)]
pub enum ExecStrategyName {
    MainnetStrategy,
    BaseStrategy,
    UnichainStrategy,
}

/// =============================================================================
/// @function: as_str
/// @description: Convert execution strategy name to string representation
/// @return &'static str: String representation of the strategy name
/// =============================================================================
impl ExecStrategyName {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExecStrategyName::MainnetStrategy => "Mainnet_Strategy",
            ExecStrategyName::BaseStrategy => "Base_Strategy",
            ExecStrategyName::UnichainStrategy => "Unichain_Strategy",
        }
    }
}

/// =============================================================================
/// @struct: ExecStrategyFactory
/// @description: Factory for creating execution strategies based on network configuration
/// =============================================================================
pub struct ExecStrategyFactory;

/// =============================================================================
/// @function: create
/// @description: Create the appropriate execution strategy based on network name
/// @param network: Network name as string (e.g., "ethereum", "base", "unichain")
/// @return Box<dyn ExecStrategy>: Boxed execution strategy instance
/// @behavior: Panics if network name is not recognized
/// =============================================================================
impl ExecStrategyFactory {
    pub fn create(network: &str) -> Box<dyn ExecStrategy> {
        match NetworkName::from_str(network) {
            Some(NetworkName::Ethereum) => Box::new(chain::mainnet::MainnetExec::new()),
            Some(NetworkName::Base) => Box::new(chain::base::BaseExec::new()),
            Some(NetworkName::Unichain) => Box::new(chain::unichain::UnichainExec::new()),
            None => panic!("Unknown network '{}', please check the network name in the config file", network),
        }
    }
}

/// =============================================================================
/// @trait: ExecStrategy
/// @description: Trait defining the interface for execution strategies
/// @methods:
/// - name: Get strategy name for logging
/// - pre_hook: Pre-execution hook
/// - post_hook: Post-execution hook
/// - execute: Main execution orchestration
/// - simulate: Transaction simulation
/// - broadcast: Transaction broadcasting
/// =============================================================================
#[async_trait]
pub trait ExecStrategy: Send + Sync {
    /// =============================================================================
    /// @function: name
    /// @description: Get the strategy name for logging purposes
    /// @return String: Strategy name as string
    /// =============================================================================
    fn name(&self) -> String;

    /// =============================================================================
    /// @function: pre_hook
    /// @description: Pre-execution hook called before transaction execution
    /// @param _config: Market maker configuration (unused parameter)
    /// @behavior: Logs default pre-execution message
    /// =============================================================================
    async fn pre_hook(&self) {
        tracing::info!("{} default_pre_exec_hook", self.name());
    }

    /// =============================================================================
    /// @function: post_hook
    /// @description: Post-execution hook called after transaction execution
    /// @param config: Market maker configuration
    /// @param trades: Vector of executed trades
    /// @param identifier: Instance identifier for trade tracking
    /// @behavior: Publishes trade events if configured and trades were successful
    /// =============================================================================
    async fn post_hook(&self, config: &MarketMakerConfig, trades: Vec<Trade>, identifier: String) {
        tracing::info!("{}: default_post_exec_hook", self.name());
        if config.publish_events {
            tracing::info!("Saving trades for instance identifier: {}", identifier);
            for trade in trades {
                let _ = crate::data::r#pub::trade(NewTradeMessage {
                    identifier: identifier.clone(), // Use passed identifier for trade tracking
                    data: trade.metadata.clone(),
                });
            }
        }
    }

    /// =============================================================================
    /// @function: default execute
    /// @description: Execute the prepared transactions with full orchestration
    /// @param config: Market maker configuration
    /// @param prepared: Vector of trades to execute
    /// @param env: Environment configuration
    /// @param identifier: Instance identifier
    /// @return Result<Vec<Trade>, String>: Executed trades or error
    /// @behavior: Orchestrates simulation, broadcasting, and status updates
    /// =============================================================================
    async fn execute(&self, config: MarketMakerConfig, prepared: Vec<Trade>, env: EnvConfig, identifier: String) -> Result<Vec<Trade>, String> {
        self.pre_hook().await;
        tracing::info!("{} Executing {} trades", self.name(), prepared.len());
        let mut trades = if config.skip_simulation {
            tracing::info!("🚀 Skipping simulation - direct execution enabled");
            prepared.clone()
        } else {
            let mut updated = prepared.clone();
            let smd = self.simulate(config.clone(), updated.clone(), env.clone()).await?;
            for (x, smd) in smd.iter().enumerate() {
                updated[x].metadata.simulation = Some(smd.clone());
                if !smd.status {
                    updated[x].metadata.status = TradeStatus::SimulationFailed;
                } else {
                    updated[x].metadata.status = TradeStatus::SimulationSucceeded;
                }
            }
            updated
        };

        let bd = self.broadcast(trades.clone(), config.clone(), env).await?;
        for (x, bd) in bd.iter().enumerate() {
            trades[x].metadata.broadcast = Some(bd.clone());
            if bd.broadcast_error.is_some() {
                trades[x].metadata.status = TradeStatus::BroadcastFailed;
            } else {
                trades[x].metadata.status = TradeStatus::BroadcastSucceeded;
            }
        }

        self.post_hook(&config, trades.clone(), identifier).await;
        Ok(trades)
    }

    /// =============================================================================
    /// @function: simulate
    /// @description: Simulate transactions to validate they will succeed before execution
    /// @param config: Market maker configuration
    /// @param trades: Vector of trades to simulate
    /// @param env: Environment configuration
    /// @return Result<Vec<SimulatedData>, String>: Simulated results or error (for swap, not for approval)
    /// @behavior: Performs EVM simulation for each trade
    /// =============================================================================
    /// Pure EVM simulation, no bundle, etc.
    async fn simulate(&self, config: MarketMakerConfig, trades: Vec<Trade>, env: EnvConfig) -> Result<Vec<SimulatedData>, String> {
        tracing::info!("{}: Simulating {} trades", self.name(), trades.len());
        // tracing::debug!("default_simulate: {} trades", trades.len());
        let chain = get_alloy_chain(config.network_name.as_str().to_string()).expect("Failed to get alloy chain");
        let rpc = config.rpc_url.parse::<url::Url>().unwrap().clone(); // ! Custom per network
        let pk = env.wallet_private_key.clone();
        let wallet = PrivateKeySigner::from_bytes(&B256::from_str(&pk).expect("Failed to convert swapper pk to B256")).expect("Failed to private key signer");
        tracing::debug!("Wallet configured: {:?}", wallet.address().to_string().to_lowercase());
        let signer = alloy::network::EthereumWallet::from(wallet.clone());
        let provider = ProviderBuilder::new().with_chain(chain).wallet(signer.clone()).on_http(rpc.clone());

        let mut output = vec![];
        for tx in trades.iter() {
            let time = std::time::Instant::now();
            let _simulation_start = std::time::SystemTime::now();
            let mut calls = vec![];
            if let Some(approval) = &tx.approve {
                calls.push(approval.clone());
            }
            calls.push(tx.swap.clone());
            let payload = SimulatePayload {
                block_state_calls: vec![SimBlock {
                    block_overrides: None,
                    state_overrides: None,
                    calls,
                }],
                trace_transfers: true,
                validation: true,
                return_full_transactions: true,
            };
            let mut smd = SimulatedData::default();
            match provider.simulate(&payload).await {
                Ok(output) => {
                    let now: std::time::SystemTime = std::time::SystemTime::now();
                    let simulated_at_ms = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                    smd.simulated_at_ms = simulated_at_ms;
                    for block in output.iter() {
                        tracing::trace!("🔮 Simulated on block #{} ...", block.inner.header.number);
                        match block.calls.len() {
                            1 => {
                                // Swap only
                                tracing::trace!("   => No approval needed, only swap");
                                let swap = &block.calls[0];
                                let took = time.elapsed().as_millis();
                                smd.simulated_took_ms = took;
                                smd.estimated_gas = swap.gas_used as u128;
                                smd.status = swap.status;

                                if !swap.status {
                                    let reason = swap.error.clone().unwrap().message;
                                    tracing::error!("   => Simulation failed on swap call. No broadcast. Reason: {}", reason);
                                    smd.error = Some(reason);
                                } else {
                                    tracing::info!("    => Swap simulation: Gas: {} | Status: {}", swap.gas_used, swap.status);
                                }
                            }
                            2 => {
                                // Approve + Swap
                                tracing::trace!(" - Approval needed, simulating both swap and approval");
                                let approval = &block.calls[0]; // Approval is ignored for now
                                let swap = &block.calls[1];
                                tracing::trace!(" - Approval simulation: Gas: {} | Status: {}", approval.gas_used, approval.status);
                                let took = time.elapsed().as_millis();
                                smd.simulated_took_ms = took;
                                smd.estimated_gas = swap.gas_used as u128;
                                smd.status = swap.status;
                                if !swap.status {
                                    let reason = swap.error.clone().unwrap().message;
                                    tracing::error!("   => Simulation failed on swap call. No broadcast. Reason: {}", reason);
                                    smd.error = Some(reason);
                                } else {
                                    tracing::info!("    => Approval simulation: Gas: {} | Status: {}", approval.gas_used, approval.status);
                                    tracing::info!("    => Swap simulation: Gas: {} | Status: {}", swap.gas_used, swap.status);
                                }
                            }
                            _ => {
                                tracing::error!("Invalid number of calls in simulation: {}", block.calls.len());
                                smd.status = false;
                                smd.error = Some(format!("Invalid number of calls: {}", block.calls.len()));
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to simulate: {:?}", e);
                    smd.status = false;
                    smd.error = Some(format!("Simulation error: {:?}", e));
                }
            };
            output.push(smd);
        }
        Ok(output)
    }

    /// =============================================================================
    /// @function: broadcast
    /// @description: Broadcast transactions (execution)
    /// @param prepared: Vector of trades to broadcast
    /// @param mmc: Market maker configuration
    /// @param env: Environment configuration
    /// @return Result<Vec<BroadcastData>, String>: Broadcast results or error
    /// @behavior: Sends approval and swap transactions for each trade (unless infinite_approval is true)
    /// =============================================================================
    async fn broadcast(&self, prepared: Vec<Trade>, mmc: MarketMakerConfig, env: EnvConfig) -> Result<Vec<BroadcastData>, String> {
        tracing::info!("{}: Broadcasting {} trades", self.name(), prepared.len());
        let alloy_chain = get_alloy_chain(mmc.network_name.as_str().to_string()).expect("Failed to get alloy chain");
        let rpc = mmc.rpc_url.parse::<url::Url>().unwrap().clone();
        let pk = env.wallet_private_key.clone();
        let wallet = PrivateKeySigner::from_bytes(&B256::from_str(&pk).expect("Failed to convert swapper pk to B256")).expect("Failed to private key signer");
        let signer = alloy::network::EthereumWallet::from(wallet.clone());
        let provider = ProviderBuilder::new().with_chain(alloy_chain).wallet(signer.clone()).on_http(rpc.clone());

        if env.testing {
            tracing::info!("Skipping broadcast ! Testing mode enabled");
            return Ok(Vec::new());
        }

        let mut output = Vec::new();
        for (x, tx) in prepared.iter().enumerate() {
            tracing::debug!("   => Tx: #{} | Broadcasting on {}", x, mmc.network_name.as_str().to_string());
            if tx.metadata.simulation.is_some() && !tx.metadata.simulation.as_ref().unwrap().status {
                tracing::error!("Simulation failed for tx: #{}, no broadcast", x);
                continue;
            }

            // Handle optional approval transaction
            let time = std::time::SystemTime::now();
            let _approval = if let Some(approval_tx) = &tx.approve {
                match provider.send_transaction(approval_tx.clone()).await {
                    Ok(approve) => {
                        let took = time.elapsed().unwrap_or_default().as_millis();
                        tracing::debug!("   => Explorer: {}tx/{} | Approval shoot took {} ms", mmc.explorer_url, approve.tx_hash(), took);
                        Some(approve)
                    }
                    Err(e) => {
                        tracing::error!("Failed to send approval transaction: {:?}", e);
                        None
                    }
                }
            } else {
                tracing::debug!("   => Skipping approval transaction (♾️  infinite_approval enabled)");
                None
            };

            let time = std::time::SystemTime::now();
            let mut bd = BroadcastData::default();
            // Send swap transaction
            match provider.send_transaction(tx.swap.clone()).await {
                Ok(swap) => {
                    let took = time.elapsed().unwrap_or_default().as_millis();
                    let now = std::time::SystemTime::now();
                    let broadcasted_at_ms = now.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis();
                    let tx_description = if tx.approve.is_some() { "Swap (+ approval)" } else { "Swap only" };
                    tracing::debug!("   => Explorer: {}tx/{} | {} broadcast took {} ms", mmc.explorer_url, swap.tx_hash(), tx_description, took);
                    bd.broadcasted_at_ms = broadcasted_at_ms;
                    bd.broadcasted_took_ms = took;
                    bd.hash = swap.tx_hash().to_string();
                    // Wait for receipt, else, it would cause nonce issues if we send the next tx too soon
                    let time = std::time::SystemTime::now();
                    match swap.get_receipt().await {
                        Ok(receipt) => {
                            let took = time.elapsed().unwrap_or_default().as_millis();
                            tracing::debug!(
                                "   => Swap transaction receipt received, tx included at block: {:?} with status: {:?} | Took {} ms to get receipt",
                                receipt.block_number,
                                receipt.status(),
                                took
                            );
                        }
                        Err(e) => {
                            tracing::error!("Failed to get swap transaction receipt: {:?}", e.to_string());
                            bd.broadcast_error = Some(format!("Failed to get swap transaction receipt: {:?}", e.to_string()));
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to send swap transaction: {:?}", e);
                    bd.broadcast_error = Some(format!("Failed to send swap transaction: {:?}", e));
                }
            }
            output.push(bd);
        }
        Ok(output)
    }
}
