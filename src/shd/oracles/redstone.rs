use alloy::{network::Ethereum, primitives::Address, providers::RootProvider, sol};
use eyre::Result;
use serde::Deserialize;
use std::sync::Arc;

// Redstone Oracle contract interface
sol! {
    #[sol(rpc)]
    interface IRedstoneOracle {
        function getPrice(bytes32 feedId) external view returns (uint256);
        function getPriceWithTimestamp(bytes32 feedId) external view returns (uint256 price, uint256 timestamp);
    }
}

// Redstone API response structure
#[derive(Debug, Deserialize)]
struct RedstoneApiResponse {
    value: f64,
    #[allow(dead_code)]
    timestamp: u64,
    #[allow(dead_code)]
    symbol: String,
}

pub struct Redstone {
    client: reqwest::Client,
    // Onchain oracle address on HyperEVM
    oracle_address: Option<Address>,
    provider: Option<Arc<RootProvider<Ethereum>>>,
}

impl Redstone {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            oracle_address: None,
            provider: None,
        }
    }

    pub fn with_onchain(oracle_address: Address, provider: Arc<RootProvider<Ethereum>>) -> Self {
        Self {
            client: reqwest::Client::new(),
            oracle_address: Some(oracle_address),
            provider: Some(provider),
        }
    }

    // Fetch price via API
    pub async fn get_price_api(&self, symbol: &str) -> Result<f64> {
        // Map token symbols to Redstone API format
        let api_symbol = match symbol.to_uppercase().as_str() {
            "HYPE" | "WHYPE" => "HYPE",
            "USDT0" | "USDT" => "USDT",
            "USDC0" | "USDC" => "USDC",
            _ => symbol,
        };

        let url = format!("https://api.redstone.finance/prices?symbol={}&provider=redstone&limit=1", api_symbol);

        tracing::debug!("Fetching from Redstone: {}", url);

        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
            .map_err(|e| eyre::eyre!("Request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(eyre::eyre!("API returned status: {}", resp.status()));
        }

        let text = resp.text().await?;

        // Parse response - Redstone returns array
        let data: Vec<RedstoneApiResponse> = serde_json::from_str(&text).map_err(|e| eyre::eyre!("Failed to parse response: {}, raw: {}", e, text))?;

        data.first().map(|p| p.value).ok_or_else(|| eyre::eyre!("No price data for {}", symbol))
    }

    // Fetch price from onchain oracle
    pub async fn get_price_onchain(&self, feed_id: &str) -> Result<f64> {
        let oracle_addr = self.oracle_address.ok_or_else(|| eyre::eyre!("Onchain oracle not configured"))?;

        let provider = self.provider.as_ref().ok_or_else(|| eyre::eyre!("Provider not configured"))?;

        // Convert feed_id to bytes32
        let feed_bytes = feed_id.as_bytes();
        let mut bytes32 = [0u8; 32];
        bytes32[..feed_bytes.len().min(32)].copy_from_slice(&feed_bytes[..feed_bytes.len().min(32)]);

        let oracle = IRedstoneOracle::new(oracle_addr, provider.clone());
        let price = oracle.getPrice(bytes32.into()).call().await?;

        // Redstone returns price with 8 decimals typically
        Ok(price.to::<u128>() as f64 / 1e8)
    }

    // Unified price fetching - tries API first, falls back to onchain
    pub async fn get_price(&self, symbol: &str) -> Result<f64> {
        // Try API first
        match self.get_price_api(symbol).await {
            Ok(price) => {
                tracing::debug!("Redstone API price for {}: ${:.2}", symbol, price);
                Ok(price)
            }
            Err(api_err) => {
                tracing::warn!("Redstone API failed for {}: {}", symbol, api_err);
                // Try onchain if available
                if self.oracle_address.is_some() { self.get_price_onchain(symbol).await } else { Err(api_err) }
            }
        }
    }
}

// Convenience functions for common pairs
pub async fn fetch_btc_usd_price() -> Result<f64> {
    let redstone = Redstone::new();
    redstone.get_price("BTC").await
}

pub async fn fetch_eth_usd_price() -> Result<f64> {
    let redstone = Redstone::new();
    redstone.get_price("ETH").await
}

pub async fn fetch_hype_usd_price() -> Result<f64> {
    let redstone = Redstone::new();
    redstone.get_price("HYPE").await
}

// Fetch multiple prices in parallel
pub async fn fetch_prices(symbols: Vec<&str>) -> Result<Vec<(String, f64)>> {
    let redstone = Redstone::new();
    let mut prices = Vec::new();

    for symbol in symbols {
        match redstone.get_price(symbol).await {
            Ok(price) => {
                tracing::info!("Redstone {}/USD: ${:.2}", symbol, price);
                prices.push((symbol.to_string(), price));
            }
            Err(e) => {
                tracing::error!("Failed to fetch {} price from Redstone: {}", symbol, e);
            }
        }
    }

    Ok(prices)
}
