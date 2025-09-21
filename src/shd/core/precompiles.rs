use alloy::{
    primitives::{Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    rpc::types::TransactionRequest,
};
use eyre::Result;
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use super::super::types::BotConfig;

// ===== PRECOMPILE ADDRESSES =====
// Based on HyperLiquid documentation and articles

pub mod addresses {
    /// L1 block number precompile - returns current L1 block number
    pub const L1_BLOCK_NUMBER: &str = "0x0000000000000000000000000000000000000809";

    /// Mark prices precompile - returns array of mark prices
    pub const MARK_PRICES: &str = "0x0000000000000000000000000000000000000806";

    /// Oracle prices precompile - returns array of oracle prices  
    pub const ORACLE_PRICES: &str = "0x0000000000000000000000000000000000000807";

    /// Spot prices precompile - returns array of spot prices
    pub const SPOT_PRICES: &str = "0x0000000000000000000000000000000000000808";

    /// Perpetual asset info precompile
    pub const PERP_ASSET_INFO: &str = "0x000000000000000000000000000000000000080a";
}

// ===== DATA STRUCTURES =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPrice {
    pub symbol: String,
    pub asset_index: u32,
    pub mark_price: f64,
    pub oracle_price: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1BlockInfo {
    pub block_number: u64,
    pub timestamp: u64,
}

// ===== PRECOMPILE PRICE READER =====

pub struct PrecompileReader {
    rpc_url: String,
}

impl PrecompileReader {
    pub fn new(config: &BotConfig) -> Self {
        tracing::info!("🔗 Initializing HyperLiquid precompile reader");
        Self {
            rpc_url: config.global.rpc_endpoint.clone(),
        }
    }

    /// Get decimal places for a specific asset index
    /// Based on HyperLiquid API meta information
    fn get_asset_decimals(&self, asset_index: u32) -> u8 {
        match asset_index {
            0 => 5,   // BTC
            1 => 4,   // ETH
            5 => 2,   // SOL
            150 => 2, // HYPE
            159 => 2, // HYPE
            _ => 8,   // Default fallback (most assets use 8 decimals)
        }
    }

    /// Get current L1 block number
    /// @description: Fetch current Ethereum Layer 1 block number from HyperLiquid precompile
    /// @return Result<u64>: Current L1 block number or error if precompile call fails
    /// =============================================================================
    pub async fn get_l1_block_number(&self) -> Result<u64> {
        tracing::info!("📊 Fetching L1 block number from precompile...");

        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);

        let precompile_addr = Address::from_str(addresses::L1_BLOCK_NUMBER)?;

        let call_result = provider.call(TransactionRequest::default().to(precompile_addr).input(Bytes::new().into())).await?;

        // L1 block number is returned as a simple integer
        let block_number = if call_result.len() >= 32 {
            // Convert first 32 bytes to U256 then to u64
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&call_result[0..32]);
            U256::from_be_bytes(bytes).to::<u64>()
        } else if call_result.len() >= 8 {
            // Try as u64 directly
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(&call_result[0..8]);
            u64::from_be_bytes(bytes)
        } else {
            return Err(eyre::eyre!("Invalid L1 block number response length: {}", call_result.len()));
        };

        tracing::info!("✅ L1 Block Number: {}", block_number);
        Ok(block_number)
    }

    /// Get mark price for a specific token by index
    pub async fn get_mark_price_by_index(&self, token_index: u32) -> Result<f64> {
        tracing::info!("📊 Fetching mark price for token index {}...", token_index);

        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let precompile_addr = Address::from_str(addresses::MARK_PRICES)?;

        // Create calldata: token index as 32-byte word
        let mut calldata = [0u8; 32];
        let index_bytes = token_index.to_be_bytes();
        calldata[28..32].copy_from_slice(&index_bytes);

        let call_result = provider.call(TransactionRequest::default().to(precompile_addr).input(calldata.to_vec().into())).await?;

        if call_result.len() >= 32 {
            let mut price_bytes = [0u8; 32];
            price_bytes.copy_from_slice(&call_result[0..32]);
            let price_u256 = U256::from_be_bytes(price_bytes);

            // Check if price is zero (token doesn't exist at this index)
            if price_u256.is_zero() {
                return Err(eyre::eyre!("No mark price available for index {} (returned 0x00...)", token_index));
            }

            // Convert perp prices using HyperLiquid formula: divide by 10^(6 - szDecimals)
            let raw_price = price_u256.to::<u128>();
            let sz_decimals = self.get_asset_decimals(token_index);

            // For perp prices: divide by 10^(6 - szDecimals)
            let exponent = 6 - (sz_decimals as i32);
            let divisor = 10_f64.powi(exponent);
            let price = raw_price as f64 / divisor;
            tracing::info!("✅ Mark price for index {} (raw={}, exp={}): ${:.2}", token_index, raw_price, exponent, price);
            return Ok(price);
        }

        Err(eyre::eyre!("Invalid mark price response for index {}", token_index))
    }

    /// Get oracle price for a specific token by index
    pub async fn get_oracle_price_by_index(&self, token_index: u32) -> Result<f64> {
        tracing::info!("🔮 Fetching oracle price for token index {}...", token_index);

        let provider = ProviderBuilder::new().connect_http(self.rpc_url.parse()?);
        let precompile_addr = Address::from_str(addresses::ORACLE_PRICES)?;

        // Create calldata: token index as 32-byte word
        let mut calldata = [0u8; 32];
        let index_bytes = token_index.to_be_bytes();
        calldata[28..32].copy_from_slice(&index_bytes);

        let call_result = provider.call(TransactionRequest::default().to(precompile_addr).input(calldata.to_vec().into())).await?;

        if call_result.len() >= 32 {
            let mut price_bytes = [0u8; 32];
            price_bytes.copy_from_slice(&call_result[0..32]);
            let price_u256 = U256::from_be_bytes(price_bytes);

            // Check if price is zero (token doesn't exist at this index)
            if price_u256.is_zero() {
                return Err(eyre::eyre!("No oracle price available for index {} (returned 0x00...)", token_index));
            }

            // Convert perp prices using HyperLiquid formula: divide by 10^(6 - szDecimals)
            let raw_price = price_u256.to::<u128>();
            let sz_decimals = self.get_asset_decimals(token_index);

            // For perp prices: divide by 10^(6 - szDecimals)
            let exponent = 6 - (sz_decimals as i32);
            let divisor = 10_f64.powi(exponent);
            let price = raw_price as f64 / divisor;
            tracing::info!("✅ Oracle price for index {} (raw={}, exp={}): ${:.2}", token_index, raw_price, exponent, price);
            return Ok(price);
        }

        Err(eyre::eyre!("Invalid oracle price response for index {}", token_index))
    }

    /// Get specific token price by asset index
    /// Based on articles: asset index 0 = BTC, 1 = ETH, etc.
    /// @description: Fetch complete price data (mark and oracle) for specific asset index
    /// @param asset_index: HyperLiquid asset index (0=BTC, 1=ETH, 159=HYPE, etc.)
    /// @return Result<TokenPrice>: Complete price structure with symbol and both price types
    /// =============================================================================
    pub async fn get_token_price_by_index(&self, asset_index: u32) -> Result<TokenPrice> {
        tracing::info!("💰 Fetching price for asset index {}", asset_index);

        let mark_price = self.get_mark_price_by_index(asset_index).await?;
        let oracle_price = self.get_oracle_price_by_index(asset_index).await?;

        // Map common asset indices to symbols (fallback for when API is not available)
        let symbol = match asset_index {
            0 => "BTC".to_string(),
            1 => "ETH".to_string(),
            5 => "SOL".to_string(),
            150 => "HYPE".to_string(), // HYPE
            159 => "HYPE".to_string(), // HYPE
            _ => format!("ASSET_{}", asset_index),
        };

        let token_price = TokenPrice {
            symbol,
            asset_index,
            mark_price,
            oracle_price,
        };

        tracing::info!("💰 {} (index {}): Mark=${}, Oracle=${}", token_price.symbol, asset_index, mark_price, oracle_price);

        Ok(token_price)
    }

    /// Get HYPE token price (try various indices)
    pub async fn get_hype_price(&self) -> Result<TokenPrice> {
        // Try multiple potential indices for HYPE
        // Note: HYPE might be at different indices or not available via precompiles
        for potential_index in [159, 150] {
            match self.get_token_price_by_index(potential_index).await {
                Ok(price) if price.mark_price > 0.0 || price.oracle_price > 0.0 => {
                    tracing::info!("Found HYPE at index {}: mark=${:.2}, oracle=${:.2}", potential_index, price.mark_price, price.oracle_price);
                    return Ok(TokenPrice {
                        symbol: "HYPE".to_string(),
                        asset_index: potential_index,
                        mark_price: price.mark_price,
                        oracle_price: price.oracle_price,
                    });
                }
                Ok(_) => {
                    tracing::debug!("Index {} returned zero prices", potential_index);
                }
                Err(e) => {
                    tracing::debug!("Index {} failed: {}", potential_index, e);
                }
            }
        }

        Err(eyre::eyre!("HYPE price not available via Hypercore precompiles"))
    }

    /// Get ETH token price (commonly at index 1)
    pub async fn get_eth_price(&self) -> Result<TokenPrice> {
        self.get_token_price_by_index(1).await
    }

    /// Get BTC token price (commonly at index 0)
    pub async fn get_btc_price(&self) -> Result<TokenPrice> {
        self.get_token_price_by_index(0).await
    }

    /// Get prices for common tokens (first 20 indices)
    pub async fn get_common_prices(&self) -> Result<Vec<TokenPrice>> {
        let mut all_prices = Vec::new();

        // Query first 20 tokens (most common ones)
        for i in 0..20 {
            match self.get_token_price_by_index(i).await {
                Ok(token_price) if token_price.mark_price > 0.0 => {
                    all_prices.push(token_price);
                }
                _ => {
                    // Skip tokens with no price or errors
                }
            }
        }

        tracing::info!("✅ Retrieved {} token prices via precompiles", all_prices.len());
        Ok(all_prices)
    }

    /// Scan for all available token prices (useful for finding HYPE index)
    pub async fn scan_for_tokens(&self, start: u32, end: u32) -> Result<Vec<(u32, f64, f64)>> {
        let mut found = Vec::new();

        tracing::info!("Scanning precompiles from index {} to {}", start, end);

        for i in start..=end {
            match self.get_mark_price_by_index(i).await {
                Ok(mark_price) if mark_price > 0.0 => {
                    let oracle_price = self.get_oracle_price_by_index(i).await.unwrap_or(0.0);
                    tracing::info!("Found token at index {}: mark=${:.2}, oracle=${:.2}", i, mark_price, oracle_price);
                    found.push((i, mark_price, oracle_price));
                }
                _ => {}
            }
        }

        tracing::info!("Found {} tokens with prices", found.len());
        Ok(found)
    }
}
