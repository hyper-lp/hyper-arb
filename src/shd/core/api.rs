use eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ===== UTILITY FUNCTIONS =====

/// Format asset index as 32-byte padded hex string for HyperCore smart contract calls
///
/// This converts the token index to the proper format for smart contract interactions:
/// - Converts decimal index to hexadecimal
/// - Pads to 32 bytes (64 hex characters) with leading zeros
/// - Prefixes with "0x"
///
/// Example: index 5 -> "0x0000000000000000000000000000000000000000000000000000000000000005"
pub fn format_hypercore_address(asset_index: u32) -> String {
    format!("0x{:064x}", asset_index)
}

// ===== DATA STRUCTURES =====

/// Token metadata from HyperLiquid meta API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyperToken {
    #[serde(default)]
    pub asset_index: u32, // Index in the API response array (we'll set this manually)
    pub name: String, // Token symbol (e.g., "BTC", "ETH", "HYPE")
    #[serde(rename = "szDecimals")]
    pub sz_decimals: u8, // Size decimals for trading
    #[serde(rename = "marginTableId", default)]
    pub margin_table_id: Option<u32>, // Margin table ID from API
    #[serde(rename = "maxLeverage", default)]
    pub max_leverage: Option<u32>, // Maximum leverage for this asset
    #[serde(default)]
    pub is_delisted: Option<bool>, // Whether the token is delisted
    #[serde(skip)]
    pub hypercore_address: String, // HyperCore address for smart contract calls (32-byte padded hex)
}

/// Meta API response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaResponse {
    pub universe: Vec<HyperToken>,
}

// ===== API CLIENT =====

pub struct HyperLiquidAPI {
    api_url: String,
    client: reqwest::Client,
}

impl HyperLiquidAPI {
    pub fn new(api_endpoint: &str) -> Self {
        Self {
            api_url: format!("{}/info", api_endpoint),
            client: reqwest::Client::new(),
        }
    }

    pub fn mainnet() -> Self {
        Self::new("https://api.hyperliquid.xyz")
    }

    /// Get token metadata from HyperLiquid meta API
    /// Equivalent to: curl -X POST https://api.hyperliquid.xyz/info -H "Content-Type: application/json" -d '{"type": "meta"}'
    pub async fn get_token_metadata(&self) -> Result<Vec<HyperToken>> {
        tracing::info!("📋 Fetching token metadata from HyperLiquid API...");

        let payload = json!({
            "type": "meta"
        });

        let response = self.client
            .post(&self.api_url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(eyre::eyre!("API request failed with status: {}", response.status()));
        }

        let meta_response: MetaResponse = response.json().await?;

        // Add asset_index and hypercore_address to each token (their position in the array)
        let mut tokens_with_index = Vec::new();
        for (index, mut token) in meta_response.universe.into_iter().enumerate() {
            token.asset_index = index as u32;
            token.hypercore_address = format_hypercore_address(index as u32);
            tokens_with_index.push(token);
        }

        tracing::info!("✅ Retrieved metadata for {} tokens from API", tokens_with_index.len());

        // Log some sample tokens with HyperCore addresses
        tracing::info!("📊 Sample tokens:");
        for token in tokens_with_index.iter().take(5) {
            tracing::info!(
                "   {}: {} (index: {}, decimals: {}, hypercore: {})",
                token.asset_index,
                token.name,
                token.asset_index,
                token.sz_decimals,
                &token.hypercore_address[..10]
            ); // Show first 10 chars of address
        }
        if tokens_with_index.len() > 5 {
            tracing::info!("   ... and {} more tokens", tokens_with_index.len() - 5);
        }

        Ok(tokens_with_index)
    }

    /// Get specific token by symbol
    pub async fn get_token_by_symbol(&self, symbol: &str) -> Result<Option<HyperToken>> {
        let tokens = self.get_token_metadata().await?;

        let token = tokens.into_iter().find(|t| t.name.eq_ignore_ascii_case(symbol));

        match &token {
            Some(t) => {
                tracing::info!("✅ Found token {}: index={}, decimals={}", t.name, t.asset_index, t.sz_decimals);
            }
            None => {
                tracing::warn!("⚠️ Token '{}' not found in API metadata", symbol);
            }
        }

        Ok(token)
    }

    /// Get token by asset index
    pub async fn get_token_by_index(&self, asset_index: u32) -> Result<Option<HyperToken>> {
        let tokens = self.get_token_metadata().await?;

        let token = tokens.into_iter().find(|t| t.asset_index == asset_index);

        match &token {
            Some(t) => {
                tracing::info!("✅ Found token at index {}: {} (decimals={})", asset_index, t.name, t.sz_decimals);
            }
            None => {
                tracing::warn!("⚠️ No token found at asset index {}", asset_index);
            }
        }

        Ok(token)
    }

    /// Get HYPE token metadata
    pub async fn get_hype_token(&self) -> Result<Option<HyperToken>> {
        self.get_token_by_symbol("HYPE").await
    }

    /// Get ETH token metadata  
    pub async fn get_eth_token(&self) -> Result<Option<HyperToken>> {
        self.get_token_by_symbol("ETH").await
    }

    /// Get BTC token metadata
    pub async fn get_btc_token(&self) -> Result<Option<HyperToken>> {
        self.get_token_by_symbol("BTC").await
    }

    /// Get tokens for multiple symbols
    pub async fn get_tokens_by_symbols(&self, symbols: &[&str]) -> Result<Vec<HyperToken>> {
        let all_tokens = self.get_token_metadata().await?;
        let mut found_tokens = Vec::new();

        for symbol in symbols {
            if let Some(token) = all_tokens.iter().find(|t| t.name.eq_ignore_ascii_case(symbol)) {
                found_tokens.push(token.clone());
                tracing::info!("✅ Found {}: index={}, decimals={}", token.name, token.asset_index, token.sz_decimals);
            } else {
                tracing::warn!("⚠️ Token '{}' not found", symbol);
            }
        }

        Ok(found_tokens)
    }

    /// Get all mid prices (mark prices) for perpetuals
    pub async fn get_all_mids(&self) -> Result<std::collections::HashMap<String, String>> {
        let payload = json!({
            "type": "allMids"
        });

        let resp = self.client
            .post(&self.api_url)
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(eyre::eyre!("API request failed with status: {}", resp.status()));
        }

        // Parse directly as HashMap since the response is a flat object with symbol keys
        let data: std::collections::HashMap<String, String> = resp.json().await?;
        Ok(data)
    }

    /// Get price for specific asset
    pub async fn get_price(&self, symbol: &str) -> Result<f64> {
        // Get all mid prices
        let mids = self.get_all_mids().await?;
        
        let price_str = mids.get(symbol)
            .ok_or_else(|| eyre::eyre!("Price not found for symbol: {}", symbol))?;
        
        let price = price_str.parse::<f64>()
            .map_err(|e| eyre::eyre!("Failed to parse price: {}", e))?;
        
        Ok(price)
    }

    /// Get BTC price
    pub async fn get_btc_price(&self) -> Result<f64> {
        self.get_price("BTC").await
    }

    /// Get ETH price  
    pub async fn get_eth_price(&self) -> Result<f64> {
        self.get_price("ETH").await
    }

    /// Get HYPE price
    pub async fn get_hype_price(&self) -> Result<f64> {
        self.get_price("HYPE").await
    }

    /// Get multiple prices at once
    pub async fn get_prices(&self, symbols: Vec<&str>) -> Result<std::collections::HashMap<String, f64>> {
        let mids = self.get_all_mids().await?;
        let mut prices = std::collections::HashMap::new();

        for symbol in symbols {
            if let Some(price_str) = mids.get(symbol) {
                if let Ok(price) = price_str.parse::<f64>() {
                    prices.insert(symbol.to_string(), price);
                    tracing::info!("HyperLiquid API {}: ${:.2}", symbol, price);
                }
            }
        }

        Ok(prices)
    }

    /// Print comprehensive token summary
    pub async fn print_token_summary(&self, symbols: &[&str]) -> Result<()> {
        let tokens = self.get_tokens_by_symbols(symbols).await?;

        tracing::info!("==================== Token Summary ====================");
        for token in &tokens {
            tracing::info!("🪙 {} (Asset Index: {})", token.name, token.asset_index);
            tracing::info!("   Decimals: {}", token.sz_decimals);
            tracing::info!("   HyperCore Address: {}", token.hypercore_address);

            if let Some(margin_id) = token.margin_table_id {
                tracing::info!("   Margin Table ID: {}", margin_id);
            }

            if let Some(delisted) = token.is_delisted {
                if delisted {
                    tracing::info!("   Status: DELISTED");
                }
            }

            if let Some(max_lev) = token.max_leverage {
                tracing::info!("   Max Leverage: {}x", max_lev);
            }
            tracing::info!("");
        }
        tracing::info!("======================================================");

        Ok(())
    }
}

// Convenience functions
pub async fn fetch_btc_price() -> Result<f64> {
    let api = HyperLiquidAPI::mainnet();
    api.get_btc_price().await
}

pub async fn fetch_eth_price() -> Result<f64> {
    let api = HyperLiquidAPI::mainnet();
    api.get_eth_price().await
}

pub async fn fetch_hype_price() -> Result<f64> {
    let api = HyperLiquidAPI::mainnet();
    api.get_hype_price().await
}