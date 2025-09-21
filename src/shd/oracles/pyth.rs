use eyre::Result;
use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct PriceUpdate {
    pub parsed: Vec<ParsedPriceUpdate>,
}

#[derive(Deserialize)]
pub struct ParsedPriceUpdate {
    pub price: PriceFeed,
}

#[derive(Deserialize)]
pub struct PriceFeed {
    pub price: String,
    pub expo: i32,
}

impl PriceFeed {
    pub fn to_price_f64(&self) -> f64 {
        self.price.parse::<f64>().unwrap_or(0.0) * 10_f64.powi(self.expo)
    }
}

pub struct Pyth {
    client: Client,
}

impl Pyth {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    pub async fn get_single_price(&self, price_id: &str) -> Result<f64> {
        let url = format!(
            "https://hermes.pyth.network/v2/updates/price/latest?ids[]={}",
            price_id
        );
        let resp = self.client.get(&url).send().await?;
        let data: PriceUpdate = resp.json().await?;

        data.parsed
            .first()
            .map(|p| p.price.to_price_f64())
            .ok_or_else(|| eyre::eyre!("No price data"))
    }
}

pub struct PythPriceIds;
impl PythPriceIds {
    pub const BTC_USD: &'static str =
        "0xe62df6c8b4a85fe1a67db44dc12de5db330f7ac66b72dc658afedf0f4a415b43";
    pub const ETH_USD: &'static str =
        "0xff61491a931112ddf1bd8147cd1b641375f79f5825126d665480874634fd0ace";
    pub const SOL_USD: &'static str =
        "0xef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";
    pub const USDT_USD: &'static str =
        "0x2b89b9dc8fdf9f34709a5b106b472f0f39bb6ca9ce04b0fd7f2e971688e2e53b";
    pub const HYPE_USD: &'static str =
        "0x4279e31cc369bbcc2faf022b382b080e32a8e689ff20fbc530d2a603eb6cd98b";
}

pub async fn fetch_btc_usd_price() -> Result<f64> {
    let pyth = Pyth::new();
    pyth.get_single_price(PythPriceIds::BTC_USD).await
}

pub async fn fetch_eth_usd_price() -> Result<f64> {
    let pyth = Pyth::new();
    pyth.get_single_price(PythPriceIds::ETH_USD).await
}

pub async fn fetch_hype_usd_price() -> Result<f64> {
    let pyth = Pyth::new();
    pyth.get_single_price(PythPriceIds::HYPE_USD).await
}
