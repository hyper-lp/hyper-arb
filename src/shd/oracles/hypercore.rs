use eyre::Result;
use super::super::{
    core::api::HyperLiquidAPI,
    types::BotConfig,
};

pub struct Hypercore {
    api: HyperLiquidAPI,
}

impl Hypercore {
    pub fn new(config: &BotConfig) -> Self {
        let api_endpoint = &config.global.hyperliquid_api_endpoint;
        
        Self {
            api: HyperLiquidAPI::new(api_endpoint),
        }
    }

    // Get price using HyperLiquid API
    pub async fn get_price(&self, symbol: &str) -> Result<f64> {
        match symbol.to_uppercase().as_str() {
            "BTC" => {
                let price = self.api.get_btc_price().await?;
                tracing::info!("Hypercore API BTC price: ${:.2}", price);
                Ok(price)
            }
            "ETH" => {
                let price = self.api.get_eth_price().await?;
                tracing::info!("Hypercore API ETH price: ${:.2}", price);
                Ok(price)
            }
            "HYPE" | "WHYPE" => {
                let price = self.api.get_hype_price().await?;
                tracing::info!("Hypercore API HYPE price: ${:.2}", price);
                Ok(price)
            }
            _ => {
                // Try to get from general API
                match self.api.get_price(symbol).await {
                    Ok(price) => {
                        tracing::info!("Hypercore API {} price: ${:.2}", symbol, price);
                        Ok(price)
                    }
                    Err(_) => Err(eyre::eyre!("Hypercore doesn't support {} price", symbol))
                }
            }
        }
    }
}

// Convenience functions
pub async fn fetch_btc_usd_price(config: &BotConfig) -> Result<f64> {
    let hypercore = Hypercore::new(config);
    hypercore.get_price("BTC").await
}

pub async fn fetch_eth_usd_price(config: &BotConfig) -> Result<f64> {
    let hypercore = Hypercore::new(config);
    hypercore.get_price("ETH").await
}

pub async fn fetch_hype_usd_price(config: &BotConfig) -> Result<f64> {
    let hypercore = Hypercore::new(config);
    hypercore.get_price("HYPE").await
}