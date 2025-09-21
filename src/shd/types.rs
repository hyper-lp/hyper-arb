// Type definitions and configuration structures

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::{env, fs};

/// Environment configuration loaded from .env file
#[derive(Debug, Deserialize, Clone)]
pub struct EnvConfig {
    /// Testing mode flag
    pub testing: bool,
    /// Database URL for Prisma client
    pub database_url: String,
    /// List of wallet public keys for multi-wallet support
    pub wallet_pub_keys: Vec<String>,
    /// List of wallet private keys (must match pub_keys order)
    pub wallet_private_keys: Vec<String>,
}

impl Default for EnvConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl EnvConfig {
    pub fn new() -> Self {
        let testing = env::var("TESTING").unwrap_or_else(|_| {
            tracing::error!("Missing TESTING in environment");
            std::process::exit(1);
        });

        // Load database configuration
        let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
            tracing::error!("Missing DATABASE_URL in environment");
            std::process::exit(1);
        });

        // Load multi-wallet support
        let wallet_pub_keys_str = env::var("WALLET_PUB_KEYS").unwrap_or_else(|_| {
            tracing::error!("Missing WALLET_PUB_KEYS in environment");
            std::process::exit(1);
        });

        let wallet_private_keys_str = env::var("WALLET_PRIVATE_KEYS").unwrap_or_else(|_| {
            tracing::error!("Missing WALLET_PRIVATE_KEYS in environment");
            std::process::exit(1);
        });

        // Parse comma-separated values
        let wallet_pub_keys: Vec<String> = wallet_pub_keys_str
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .collect();

        let wallet_private_keys: Vec<String> = wallet_private_keys_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();

        // Load hyperdrive webhook (required)

        let output = Self {
            testing: testing == "true",
            database_url,
            wallet_pub_keys,
            wallet_private_keys,
        };

        output
            .validate_wallets()
            .expect("Invalid wallet configuration");
        output.print();
        output
    }

    pub fn print(&self) {
        tracing::info!("Loaded environment variables:");
        tracing::info!("   Testing = {}", self.testing);

        tracing::info!("   Database URL = 🗄️ (size: {})", self.database_url.len());
        tracing::info!(
            "   Multi-wallet: {} wallets configured",
            self.wallet_pub_keys.len()
        );
    }

    /// Validates that public keys and private keys match by count and that each private key
    /// derives to the corresponding public key address
    pub fn validate_wallets(&self) -> Result<(), String> {
        // Check that we have the same number of public and private keys
        if self.wallet_pub_keys.len() != self.wallet_private_keys.len() {
            return Err(format!(
                "Mismatch between public keys count ({}) and private keys count ({})",
                self.wallet_pub_keys.len(),
                self.wallet_private_keys.len()
            ));
        }

        // Check that we have at least one wallet
        if self.wallet_pub_keys.is_empty() {
            return Err(
                "No wallets configured - WALLET_PUB_KEYS and WALLET_PRIVATE_KEYS cannot be empty"
                    .to_string(),
            );
        }

        // Validate each wallet pair
        for (i, (pub_key, priv_key)) in self
            .wallet_pub_keys
            .iter()
            .zip(self.wallet_private_keys.iter())
            .enumerate()
        {
            // Validate public key format (0x + 40 hex chars)
            if !pub_key.starts_with("0x") || pub_key.len() != 42 {
                return Err(format!(
                    "Wallet {}: public key '{}' must be a valid Ethereum address",
                    i, pub_key
                ));
            }

            // Try to create a signer from the private key
            let signer = match PrivateKeySigner::from_str(priv_key) {
                Ok(s) => s,
                Err(e) => return Err(format!("Wallet {}: invalid private key format: {}", i, e)),
            };

            // Get the address derived from the private key
            let derived_address = format!("0x{:x}", signer.address()).to_lowercase();

            // Check if the derived address matches the provided public key
            if derived_address != pub_key.to_lowercase() {
                return Err(format!(
                    "Wallet {}: private key derives to address '{}' but public key is '{}'",
                    i, derived_address, pub_key
                ));
            }
        }

        tracing::info!(
            "✅ All {} wallets validated successfully",
            self.wallet_pub_keys.len()
        );
        Ok(())
    }

    /// Find the private key for a given vault address
    /// Returns None if no matching wallet is found
    pub fn get_private_key_for_address(&self, vault_address: &str) -> Option<&String> {
        let normalized_vault = vault_address.to_lowercase();

        for (i, pub_key) in self.wallet_pub_keys.iter().enumerate() {
            if pub_key.to_lowercase() == normalized_vault {
                return Some(&self.wallet_private_keys[i]);
            }
        }

        None
    }

    /// Create a wallet signer for a given vault address
    /// Returns None if no matching wallet is found or if signer creation fails
    pub fn get_signer_for_address(&self, vault_address: &str) -> Option<PrivateKeySigner> {
        if let Some(private_key) = self.get_private_key_for_address(vault_address) {
            PrivateKeySigner::from_str(private_key).ok()
        } else {
            None
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct BotConfig {
    pub global: GlobalConfig,
    pub hyperevm: HyperEvmConfig,
    pub gas: GasConfig,
    pub dex: Vec<DexConfig>,
    pub targets: Vec<ArbTargets>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GlobalConfig {
    pub network_name: String,             // Network identifier
    pub rpc_endpoint: String,             // RPC endpoint for blockchain transactions
    pub websocket_endpoint: String,       // WebSocket endpoint for real-time events
    pub hyperliquid_api_endpoint: String, // Hyperliquid API endpoint
    pub explorer_base_url: String,        // Blockchain explorer URL
}

#[derive(Debug, Deserialize, Clone)]
pub struct HyperEvmConfig {
    pub core_bridge_contract: String, // CoreWriter contract for cross-chain transfers
    pub wrapped_hype_token_address: String, // Wrapped HYPE token address on HyperEVM (like WETH)
    pub bridge_hype_token_address: String, // HYPE token address for L1 bridging operations
    pub liqd_multi_hop_router_address: String, // Liquid Labs multi-hop router for DEX aggregation
    pub liquidswap_api_endpoint: String, // Liquid Labs API endpoint (required in config)
}

#[derive(Debug, Deserialize, Clone)]
pub struct GasConfig {
    pub gas_estimate_multiplier: f64, // Multiplier for gas estimates (e.g., 1.5 = 50% buffer)
    pub slippage_tolerance_percent: f64, // Slippage tolerance in percent (e.g., 5.0 = 5%)
    pub native_hype_reserve_amount: f64, // Native HYPE reserve amount to keep when wrapping (e.g., 0.1 HYPE)
    pub max_gas_price_gwei: f64, // Maximum gas price in gwei above which rebalancing is skipped (e.g., 3.0 = 3 gwei)
    pub gas_price_multiplier: f64, // Gas price multiplier for transactions (e.g., 1.5 = 50% increase)
}

#[derive(Debug, Deserialize, Clone)]
pub struct DexConfig {
    pub name: String,
    pub version: String,
    pub factory: String,
    pub router: String,
    pub quoter: String,
    pub position_manager: String, // Position manager address (required)
}

#[derive(Debug, Deserialize, Clone)]
pub struct ArbTargets {
    pub vault_name: String,
    pub address: String,
    pub base_token: String,
    pub quote_token: String,
    pub hyperswap_pools: Vec<String>,
    pub prjx_pools: Vec<String>,
    pub min_watch_spread_bps: f64,
    pub min_executable_spread_bps: f64,
    pub max_slippage_pct: f64,
    pub max_inventory_ratio: f64,
    pub tx_gas_limit: u64,
    pub poll_interval_ms: u64,
    pub publish_events: bool,
    pub skip_simulation: bool,
    pub infinite_approval: bool,
}

impl BotConfig {
    pub fn print(&self) {
        tracing::debug!(" >>> Config <<<");
        tracing::debug!("  Network:                {}", self.global.network_name);
        tracing::debug!("  RPC Endpoint:           {}", self.global.rpc_endpoint);
        tracing::debug!(
            "  WebSocket Endpoint:     {}",
            self.global.websocket_endpoint
        );
        tracing::debug!(
            "  Hyperliquid API:        {}",
            self.global.hyperliquid_api_endpoint
        );
        tracing::debug!(
            "  Explorer URL:           {}",
            self.global.explorer_base_url
        );
        tracing::debug!(
            "  Core Bridge Contract:   {}",
            self.hyperevm.core_bridge_contract
        );
        tracing::debug!(
            "  Wrapped HYPE Address:   {}",
            self.hyperevm.wrapped_hype_token_address
        );
        tracing::debug!(
            "  Bridge HYPE Address:    {}",
            self.hyperevm.bridge_hype_token_address
        );

        tracing::debug!(
            "  Liquid Labs Router:     {}",
            self.hyperevm.liqd_multi_hop_router_address
        );
        tracing::debug!(
            "  Liquid Labs API:        {}",
            self.hyperevm.liquidswap_api_endpoint
        );
        tracing::debug!(
            "  Gas Estimate Multiplier: {}x",
            self.gas.gas_estimate_multiplier
        );
        tracing::debug!(
            "  Slippage Tolerance:     {}%",
            self.gas.slippage_tolerance_percent
        );
        tracing::debug!(
            "  Native HYPE Reserve:    {} HYPE",
            self.gas.native_hype_reserve_amount
        );

        if !self.dex.is_empty() {
            tracing::debug!("  DEX Configurations:");
            for dex in &self.dex {
                tracing::debug!(
                    "   - {} ({}): Factory={}, Router={}",
                    dex.name,
                    dex.version,
                    dex.factory,
                    dex.router
                );
            }
        }

        if !self.targets.is_empty() {
            tracing::debug!("  Targets Configurations:");
            for track in &self.targets {
                tracing::debug!("   ╔═══ Target: {} ═══╗", track.vault_name);
                tracing::debug!("   ║ Address: {}", track.address);
                tracing::debug!("   ║ Pair: {}/{}", track.base_token, track.quote_token);
                tracing::debug!("   ║ Hyperswap Pools: {:?}", track.hyperswap_pools);
                tracing::debug!("   ║ ProjectX Pools: {:?}", track.prjx_pools);
                tracing::debug!("   ║ Watch Spread: {} bps", track.min_watch_spread_bps);
                tracing::debug!("   ║ Exec Spread: {} bps", track.min_executable_spread_bps);
                tracing::debug!("   ║ Max Slippage: {}%", track.max_slippage_pct * 100.0);
                tracing::debug!("   ║ Max Inventory: {}%", track.max_inventory_ratio * 100.0);
                tracing::debug!("   ║ Gas Limit: {}", track.tx_gas_limit);
                tracing::debug!("   ║ Poll Interval: {} ms", track.poll_interval_ms);
                tracing::debug!("   ║ Publish Events: {}", track.publish_events);
                tracing::debug!("   ║ Skip Simulation: {}", track.skip_simulation);
                tracing::debug!("   ║ Infinite Approval: {}", track.infinite_approval);
                tracing::debug!("   ╚════════════════════╝");
            }
        }
        tracing::debug!(" >>> End of Config <<<");
    }

    pub fn validate(&self, env_config: Option<&EnvConfig>) -> Result<(), String> {
        if self.global.network_name.is_empty() {
            return Err("Network name cannot be empty".to_string());
        }
        if self.global.rpc_endpoint.is_empty() {
            return Err("RPC endpoint cannot be empty".to_string());
        }
        if self.global.websocket_endpoint.is_empty() {
            return Err("WebSocket endpoint cannot be empty".to_string());
        }
        if self.global.hyperliquid_api_endpoint.is_empty() {
            return Err("Hyperliquid API endpoint cannot be empty".to_string());
        }
        if self.hyperevm.core_bridge_contract.is_empty() {
            return Err("Core bridge contract address cannot be empty".to_string());
        }
        if self.hyperevm.wrapped_hype_token_address.is_empty() {
            return Err("Wrapped HYPE token address cannot be empty".to_string());
        }
        if self.hyperevm.bridge_hype_token_address.is_empty() {
            return Err("Bridge HYPE token address cannot be empty".to_string());
        }

        if self.hyperevm.liqd_multi_hop_router_address.is_empty() {
            return Err("Liquid Labs multi-hop router address cannot be empty".to_string());
        }

        // Validate HyperEVM addresses are properly formatted (0x + 40 hex chars)
        if !self.hyperevm.core_bridge_contract.starts_with("0x")
            || self.hyperevm.core_bridge_contract.len() != 42
        {
            return Err("Core bridge contract must be a valid Ethereum address".to_string());
        }
        if !self.hyperevm.wrapped_hype_token_address.starts_with("0x")
            || self.hyperevm.wrapped_hype_token_address.len() != 42
        {
            return Err("Wrapped HYPE token address must be a valid Ethereum address".to_string());
        }
        if !self.hyperevm.bridge_hype_token_address.starts_with("0x")
            || self.hyperevm.bridge_hype_token_address.len() != 42
        {
            return Err("Bridge HYPE token address must be a valid Ethereum address".to_string());
        }

        if !self
            .hyperevm
            .liqd_multi_hop_router_address
            .starts_with("0x")
            || self.hyperevm.liqd_multi_hop_router_address.len() != 42
        {
            return Err(
                "Liquid Labs multi-hop router address must be a valid Ethereum address".to_string(),
            );
        }

        // Validate Gas configuration
        if self.gas.gas_estimate_multiplier <= 0.0 {
            return Err(
                "Gas estimate multiplier must be positive (recommended: 1.5-3.0)".to_string(),
            );
        }
        if self.gas.gas_estimate_multiplier > 10.0 {
            return Err("Gas estimate multiplier is too high (recommended: 1.5-3.0)".to_string());
        }

        if self.gas.slippage_tolerance_percent <= 0.0 || self.gas.slippage_tolerance_percent > 50.0
        {
            return Err("Slippage tolerance must be between 0.1% and 50%".to_string());
        }
        if self.gas.native_hype_reserve_amount < 0.0 || self.gas.native_hype_reserve_amount > 10.0 {
            return Err("Native HYPE reserve amount must be between 0.0 and 10.0 HYPE".to_string());
        }
        if self.gas.gas_price_multiplier < 1.0 || self.gas.gas_price_multiplier > 5.0 {
            return Err("Gas price multiplier must be between 1.0 and 5.0".to_string());
        }

        // Validate DEX configurations
        for dex in &self.dex {
            if dex.name.is_empty() {
                return Err("DEX name cannot be empty".to_string());
            }
            if dex.version.is_empty() {
                return Err(format!("DEX {} version cannot be empty", dex.name));
            }

            // Only validate non-empty addresses (empty addresses indicate TODO/not configured)
            if !dex.factory.is_empty()
                && (!dex.factory.starts_with("0x") || dex.factory.len() != 42)
            {
                return Err(format!(
                    "DEX {} factory address must be a valid Ethereum address",
                    dex.name
                ));
            }
            if !dex.router.is_empty() && (!dex.router.starts_with("0x") || dex.router.len() != 42) {
                return Err(format!(
                    "DEX {} router address must be a valid Ethereum address",
                    dex.name
                ));
            }
            if !dex.quoter.is_empty() && (!dex.quoter.starts_with("0x") || dex.quoter.len() != 42) {
                return Err(format!(
                    "DEX {} quoter address must be a valid Ethereum address",
                    dex.name
                ));
            }

            // Validate optional fields if present
            if !dex.position_manager.is_empty()
                && (!dex.position_manager.starts_with("0x") || dex.position_manager.len() != 42)
            {
                return Err(format!(
                    "DEX {} position manager address must be a valid Ethereum address",
                    dex.name
                ));
            }
        }

        // Validate targets configurations
        for track in &self.targets {
            if track.vault_name.is_empty() {
                return Err("targets vault name cannot be empty".to_string());
            }
            if !track.address.starts_with("0x") || track.address.len() != 42 {
                return Err(format!(
                    "targets address for {} must be a valid Ethereum address",
                    track.vault_name
                ));
            }
        }

        // Check for duplicate addresses in targets configurations
        let mut seen_addresses = std::collections::HashSet::new();
        for track in &self.targets {
            let normalized_address = track.address.to_lowercase();
            if !seen_addresses.insert(normalized_address.clone()) {
                return Err(format!(
                    "Duplicate targets address found: '{}' ({}). Each address can only be tracked once.",
                    track.address, track.vault_name
                ));
            }
        }

        // Check for duplicate vault names in targets configurations
        let mut seen_names = std::collections::HashSet::new();
        for track in &self.targets {
            let normalized_name = track.vault_name.to_lowercase();
            if !seen_names.insert(normalized_name.clone()) {
                return Err(format!(
                    "Duplicate vault name found: '{}' (address: {}). Each vault name must be unique.",
                    track.vault_name, track.address
                ));
            }
        }

        // Validate wallet configuration if provided
        if let Some(env) = env_config {
            for track in &self.targets {
                // Check if there's a matching wallet for this vault address
                if env.get_private_key_for_address(&track.address).is_none() {
                    return Err(format!(
                        "No matching wallet found for vault address '{}' ({}). Please ensure this address is included in WALLET_PUB_KEYS",
                        track.address, track.vault_name
                    ));
                }
            }
            tracing::info!("✅ All vault addresses have matching wallets configured");
        }

        Ok(())
    }

    /// Find a DEX configuration by name
    pub fn get_dex(&self, name: &str) -> Option<&DexConfig> {
        self.dex.iter().find(|d| d.name == name)
    }

    /// Get all configured DEXs (have non-empty factory and router)
    pub fn get_configured_dexs(&self) -> Vec<&DexConfig> {
        self.dex
            .iter()
            .filter(|d| !d.factory.is_empty() && !d.router.is_empty())
            .collect()
    }

    /// Get all targets addresses
    pub fn get_tracked_addresses(&self) -> Vec<&ArbTargets> {
        self.targets.iter().collect()
    }
}

impl ArbTargets {
    /// Returns a formatted log string with targets account data
    /// Format: "vault_name-first7chars"
    /// Example: "alice-0x1a2b3c4"
    pub fn format_log_info(&self) -> String {
        let address_short = if self.address.len() >= 7 {
            &self.address[..7]
        } else {
            &self.address
        };

        format!("{}-{}", self.vault_name, address_short)
    }
}

pub fn load_bot_config(path: &str) -> BotConfig {
    let contents = fs::read_to_string(path)
        .map_err(|e| {
            tracing::error!("Failed to read config file '{}': {}", path, e);
            std::process::exit(1);
        })
        .unwrap();

    let config: BotConfig = toml::from_str(&contents)
        .map_err(|e| {
            tracing::error!("Failed to parse TOML configuration in '{}': {}", path, e);
            tracing::error!("Please ensure all required sections are present: [global], [hyperevm], [hypercore], [gas], [monitoring]");
            std::process::exit(1);
        })
        .unwrap();

    config
        .validate(None)
        .map_err(|e| {
            tracing::error!("Configuration validation failed: {}", e);
            std::process::exit(1);
        })
        .unwrap();

    config.print();
    config
}

pub fn load_bot_config_with_env(path: &str, env_config: &EnvConfig) -> BotConfig {
    let contents = fs::read_to_string(path)
        .map_err(|e| {
            tracing::error!("Failed to read config file '{}': {}", path, e);
            std::process::exit(1);
        })
        .unwrap();

    let config: BotConfig = toml::from_str(&contents)
        .map_err(|e| {
            tracing::error!("Failed to parse TOML configuration in '{}': {}", path, e);
            tracing::error!("Please ensure all required sections are present: [global], [hyperevm], [hypercore], [gas], [monitoring]");
            std::process::exit(1);
        })
        .unwrap();

    config
        .validate(Some(env_config))
        .map_err(|e| {
            tracing::error!("Configuration validation failed: {}", e);
            std::process::exit(1);
        })
        .unwrap();

    config.print();
    config
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenMetadata {
    pub name: String,
    pub sym: String,
    pub precision: u8,
    pub token: Address,
}
