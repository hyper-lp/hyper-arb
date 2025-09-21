// Utilities, notifications, and helper functions

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    str::FromStr,
    sync::Arc,
};

use alloy::{
    network::Network,
    primitives::Address,
    providers::{Provider, RootProvider},
};
use serde::{Serialize, de::DeserializeOwned};

use crate::{sol::IERC20, types::TokenMetadata};

/// Constants
pub const BASIS_POINT_DENOMINATOR: f64 = 10000.0;
pub const TICK_BASE: f64 = 1.0001;

pub fn read<T: DeserializeOwned>(file: &str) -> Vec<T> {
    let mut f = File::open(file).unwrap();
    let mut buffer = String::new();
    f.read_to_string(&mut buffer).unwrap();
    let db: Vec<T> = serde_json::from_str(&buffer).unwrap();
    db
}

pub fn save<T: Serialize>(output: Vec<T>, file: &str) {
    // log::info!("Saving to file: {}", file);
    let mut file = OpenOptions::new().create(true).write(true).truncate(true).open(file).expect("Failed to open or create file");
    let json = serde_json::to_string(&output).expect("Failed to serialize JSON");
    file.write_all(json.as_bytes()).expect("Failed to write to file");
    file.write_all(b"\n").expect("Failed to write newline to file");
    file.flush().expect("Failed to flush file");
}

// === EVM UTILITIES ===

/**
 * Get current block number from provider
 */
pub async fn block<T: Network>(provider: RootProvider<T>) -> Result<u64, String> {
    match provider.get_block_number().await {
        Ok(current) => Ok(current),
        Err(e) => Err(e.to_string()),
    }
}

/**
 * Fetch the metadata of an ERC20 token
 */
pub async fn token_metadata<T: Network>(provider: &RootProvider<T>, token: String) -> TokenMetadata {
    let client = Arc::new(provider);
    let token = Address::from_str(&token).unwrap();
    let contract = IERC20::new(token, client);
    let name = contract.name().call().await;
    let precision = contract.decimals().call().await;
    let sym = contract.symbol().call().await;
    match (name, precision, sym) {
        (Ok(name), Ok(precision), Ok(sym)) => TokenMetadata { name, precision, token, sym },
        _ => {
            tracing::warn!("🔺 Erc20::metadata: unknown name|precision|symbol");
            TokenMetadata::default()
        }
    }
}

/**
 * Format transaction link using explorer base URL
 */
pub fn format_tx_link(explorer_url: &str, tx_hash: &str) -> String {
    if explorer_url.ends_with('/') {
        format!("{}tx/{}", explorer_url, tx_hash)
    } else {
        format!("{}/tx/{}", explorer_url, tx_hash)
    }
}

// Gas benchmarks for various operations
static GAS_DECREASE_LIQUIDITY: u64 = 150_000;
static GAS_COLLECT: u64 = 100_000;
static GAS_APPROVE: u64 = 50_000;
static GAS_SWAP: u64 = 150_000;
static GAS_MINT_NEW: u64 = 400_000;
static GAS_NATIVE_TRANSFER: u64 = 21_000;
static GAS_USDT_TRANSFER: u64 = 60_000;

// Full rebalancing operation gas cost
static GAS_FULL_REBALANCE: u64 = GAS_DECREASE_LIQUIDITY + GAS_COLLECT + (2 * GAS_APPROVE) + GAS_SWAP + GAS_MINT_NEW;

/**
 * Get and log current network gas prices in native token and USD
 * Returns (gas_price_wei, gas_price_gwei, gas_price_usd_per_transfer)
 */
pub async fn log_gas_prices<T: Network>(provider: RootProvider<T>, gas_token_usd_price: f64) -> Result<(u128, f64, f64), String> {
    // Get current gas price from the network
    let gas_price_wei = provider.get_gas_price().await.map_err(|e| format!("Failed to get gas price: {}", e))?;

    // Convert to Gwei (1 Gwei = 10^9 Wei)
    let gas_price_gwei = gas_price_wei as f64 / 1e9;

    // Calculate costs in HYPE
    let native_transfer_cost_hype = (gas_price_wei as f64 * GAS_NATIVE_TRANSFER as f64) / 1e18;
    let _usdt_transfer_cost_hype = (gas_price_wei as f64 * GAS_USDT_TRANSFER as f64) / 1e18;
    let full_rebalance_cost_hype = (gas_price_wei as f64 * GAS_FULL_REBALANCE as f64) / 1e18;

    // Calculate costs in USD
    let native_transfer_cost_usd = native_transfer_cost_hype * gas_token_usd_price;
    let full_rebalance_cost_usd = full_rebalance_cost_hype * gas_token_usd_price;

    // Log the information (simplified)
    tracing::info!(
        "⛽ Gas: {:.3} Gwei | Full LP Rebalance: {:.6} HYPE (${:.2} USD)",
        gas_price_gwei,
        full_rebalance_cost_hype,
        full_rebalance_cost_usd
    );

    Ok((gas_price_wei, gas_price_gwei, native_transfer_cost_usd))
}
