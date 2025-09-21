use eyre::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

// ===== CONFIGURATION =====

/// Configuration for Hyperliquid spot balance fetcher
#[derive(Debug, Clone)]
pub struct HyperliquidConfig {
    /// API endpoint (default: "https://api.hyperliquid.xyz")
    pub api_endpoint: String,
    /// Request timeout in seconds (default: 30)
    pub timeout_secs: u64,
}

impl Default for HyperliquidConfig {
    fn default() -> Self {
        Self {
            api_endpoint: "https://api.hyperliquid.xyz".to_string(),
            timeout_secs: 30,
        }
    }
}

// ===== SPOT BALANCE STRUCTURES =====

/// Individual spot balance for a token
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpotBalance {
    /// Token symbol (e.g., "HYPE", "USDT", "USDC", "ETH", "BTC")
    pub coin: String,
    /// Total balance (including held amounts)
    pub total: String,
    /// Amount held in open orders
    pub hold: String,
    /// Available balance (total - hold)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<String>,
}

impl SpotBalance {
    /// Calculate available balance (total - hold)
    pub fn calculate_available(&mut self) {
        if let (Ok(total), Ok(hold)) = (self.total.parse::<f64>(), self.hold.parse::<f64>()) {
            self.available = Some(format!("{:.8}", total - hold));
        }
    }

    /// Get total balance as f64
    pub fn total_as_f64(&self) -> Result<f64> {
        self.total.parse()
            .map_err(|e| eyre::eyre!("Failed to parse total balance for {}: {}", self.coin, e))
    }

    /// Get hold balance as f64
    pub fn hold_as_f64(&self) -> Result<f64> {
        self.hold.parse()
            .map_err(|e| eyre::eyre!("Failed to parse hold balance for {}: {}", self.coin, e))
    }

    /// Get available balance as f64
    pub fn available_as_f64(&self) -> Result<f64> {
        if let Some(available) = &self.available {
            available.parse()
                .map_err(|e| eyre::eyre!("Failed to parse available balance for {}: {}", self.coin, e))
        } else {
            // Calculate if not already calculated
            Ok(self.total_as_f64()? - self.hold_as_f64()?)
        }
    }

    /// Check if balance is non-zero
    pub fn has_balance(&self) -> Result<bool> {
        Ok(self.total_as_f64()? > 0.0)
    }
}

/// Response from spotClearinghouseState API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpotClearinghouseState {
    /// List of spot balances
    pub balances: Vec<SpotBalanceRaw>,
    /// Withdrawable amount (optional)
    #[serde(default)]
    pub withdrawable: Option<String>,
}

/// Raw spot balance from API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpotBalanceRaw {
    pub coin: String,
    pub hold: String,
    pub total: String,
}

/// EVM contract info for bridged tokens
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EvmContractInfo {
    pub address: String,
    pub evm_extra_wei_decimals: i32,  // Can be negative
}

/// Token metadata from spotMeta API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpotToken {
    /// Token symbol
    pub name: String,
    /// Token index in the system
    pub index: u32,
    /// Token ID for transfers
    #[serde(rename = "tokenId")]
    pub token_id: String,
    /// Decimals for size calculations
    #[serde(rename = "szDecimals")]
    pub sz_decimals: u8,
    /// Decimals for wei calculations
    #[serde(rename = "weiDecimals")]
    pub wei_decimals: u8,
    /// Whether this is the canonical version of the token
    #[serde(rename = "isCanonical")]
    pub is_canonical: bool,
    /// EVM contract info (if bridged token)
    #[serde(rename = "evmContract", default)]
    pub evm_contract: Option<EvmContractInfo>,
    /// Full token name
    #[serde(rename = "fullName", default)]
    pub full_name: Option<String>,
    /// Deployer trading fee share
    #[serde(rename = "deployerTradingFeeShare", default)]
    pub deployer_trading_fee_share: Option<String>,
}

/// Response from spotMeta API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpotMetaResponse {
    pub tokens: Vec<SpotToken>,
}

// ===== TOKEN CONSTANTS =====

/// Common Hyperliquid spot tokens and their properties
pub struct TokenInfo {
    pub symbol: String,
    pub token_id: Option<u32>,
    pub evm_address: Option<String>,
    pub decimals: u8,
}

impl TokenInfo {
    /// Get common token configurations
    pub fn common_tokens() -> HashMap<String, TokenInfo> {
        let mut tokens = HashMap::new();

        // Native HYPE token
        tokens.insert(
            "HYPE".to_string(),
            TokenInfo {
                symbol: "HYPE".to_string(),
                token_id: Some(0),
                evm_address: None, // Native token
                decimals: 8,
            },
        );

        // USDT (bridged)
        tokens.insert(
            "USDT".to_string(),
            TokenInfo {
                symbol: "USDT".to_string(),
                token_id: Some(268), // 0x010C
                evm_address: Some("0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb".to_string()),
                decimals: 6,
            },
        );

        // USDC
        tokens.insert(
            "USDC".to_string(),
            TokenInfo {
                symbol: "USDC".to_string(),
                token_id: Some(1), // Usually index 1
                evm_address: None,
                decimals: 6,
            },
        );

        // ETH (bridged)
        tokens.insert(
            "ETH".to_string(),
            TokenInfo {
                symbol: "ETH".to_string(),
                token_id: Some(2), // Usually index 2
                evm_address: None,
                decimals: 8,
            },
        );

        // BTC (bridged)
        tokens.insert(
            "BTC".to_string(),
            TokenInfo {
                symbol: "BTC".to_string(),
                token_id: Some(3), // Usually index 3
                evm_address: None,
                decimals: 8,
            },
        );

        tokens
    }
}

// ===== BALANCE FETCHER =====

/// Hyperliquid spot balance fetcher
pub struct HyperliquidSpotBalances {
    config: HyperliquidConfig,
    client: reqwest::Client,
    api_url: String,
}

impl HyperliquidSpotBalances {
    /// Create new balance fetcher with default config
    pub fn new() -> Result<Self> {
        Self::with_config(HyperliquidConfig::default())
    }

    /// Create new balance fetcher with custom config
    pub fn with_config(config: HyperliquidConfig) -> Result<Self> {
        let api_url = format!("{}/info", config.api_endpoint);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;

        Ok(Self { config, client, api_url })
    }

    /// Make API request to Hyperliquid
    async fn request(&self, payload: serde_json::Value) -> Result<serde_json::Value> {
        let response = self.client
            .post(&self.api_url)
            .header("Content-Type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| eyre::eyre!("Failed to send API request: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await
                .map_err(|e| eyre::eyre!("Failed to read error response: {}", e))?;
            return Err(eyre::eyre!("API request failed with status {}: {}", status, text));
        }

        let json = response.json().await
            .map_err(|e| eyre::eyre!("Failed to parse API response as JSON: {}", e))?;
        Ok(json)
    }

    /// Get spot balances for a user address
    ///
    /// # Arguments
    /// * `user_address` - Ethereum address (0x...) of the user
    ///
    /// # Returns
    /// * Vector of spot balances for all tokens with non-zero balances
    pub async fn get_spot_balances(&self, user_address: &str) -> Result<Vec<SpotBalance>> {
        let payload = json!({
            "type": "spotClearinghouseState",
            "user": user_address
        });

        let response = self.request(payload).await?;
        let state: SpotClearinghouseState = serde_json::from_value(response)?;

        // Convert raw balances to SpotBalance with available calculation
        let mut balances: Vec<SpotBalance> = state
            .balances
            .into_iter()
            .map(|raw| {
                let mut balance = SpotBalance {
                    coin: raw.coin,
                    total: raw.total,
                    hold: raw.hold,
                    available: None,
                };
                balance.calculate_available();
                balance
            })
            .collect();

        // Sort by total balance (descending)
        balances.sort_by(|a, b| {
            match (b.total_as_f64(), a.total_as_f64()) {
                (Ok(b_total), Ok(a_total)) => b_total.partial_cmp(&a_total).unwrap_or(std::cmp::Ordering::Equal),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,  // Valid balances come first
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => std::cmp::Ordering::Equal,
            }
        });

        Ok(balances)
    }

    /// Get balances for specific tokens only
    ///
    /// # Arguments
    /// * `user_address` - Ethereum address of the user
    /// * `tokens` - List of token symbols to fetch (e.g., ["HYPE", "USDT", "USDC"])
    pub async fn get_specific_balances(&self, user_address: &str, tokens: &[&str]) -> Result<Vec<SpotBalance>> {
        let all_balances = self.get_spot_balances(user_address).await?;

        let filtered: Vec<SpotBalance> = all_balances
            .into_iter()
            .filter(|balance| tokens.iter().any(|&token| balance.coin.eq_ignore_ascii_case(token)))
            .collect();

        Ok(filtered)
    }

    /// Get balances for main tokens (HYPE, USDT, USDC, ETH, BTC)
    pub async fn get_main_balances(&self, user_address: &str) -> Result<Vec<SpotBalance>> {
        let main_tokens = ["HYPE", "USDT", "USDC", "ETH", "BTC"];
        self.get_specific_balances(user_address, &main_tokens).await
    }

    /// Get non-zero balances only
    pub async fn get_non_zero_balances(&self, user_address: &str) -> Result<Vec<SpotBalance>> {
        let balances = self.get_spot_balances(user_address).await?;
        let mut non_zero_balances = Vec::new();
        
        for balance in balances {
            if balance.has_balance()? {
                non_zero_balances.push(balance);
            }
        }
        
        Ok(non_zero_balances)
    }

    /// Get spot token metadata (global, not user-specific)
    pub async fn get_spot_tokens(&self) -> Result<Vec<SpotToken>> {
        let payload = json!({
            "type": "spotMeta"
        });

        let response = self.request(payload).await?;
        let meta: SpotMetaResponse = serde_json::from_value(response)?;

        Ok(meta.tokens)
    }

    /// Find token metadata by symbol
    pub async fn find_token_by_symbol(&self, symbol: &str) -> Result<Option<SpotToken>> {
        let tokens = self.get_spot_tokens().await?;
        Ok(tokens.into_iter().find(|t| t.name.eq_ignore_ascii_case(symbol)))
    }

    /// Get balance summary with USD values (requires price data)
    pub async fn get_balance_summary(&self, user_address: &str) -> Result<BalanceSummary> {
        let balances = self.get_non_zero_balances(user_address).await?;

        let mut summary = BalanceSummary {
            address: user_address.to_string(),
            balances,
            total_count: 0,
            timestamp: chrono::Utc::now().timestamp(),
        };

        summary.total_count = summary.balances.len();
        Ok(summary)
    }
}

// ===== SUMMARY STRUCTURES =====

/// Balance summary for a user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceSummary {
    /// User address
    pub address: String,
    /// List of non-zero balances
    pub balances: Vec<SpotBalance>,
    /// Total number of tokens with balance
    pub total_count: usize,
    /// Timestamp of the query
    pub timestamp: i64,
}

impl BalanceSummary {
    /// Print formatted balance summary
    pub fn print_summary(&self) {
        println!("\n===== Hyperliquid Spot Balances =====");
        println!("Address: {}", self.address);
        println!("Tokens with balance: {}", self.total_count);
        let timestamp_str = chrono::DateTime::from_timestamp(self.timestamp, 0)
            .map(|dt| dt.to_string())
            .unwrap_or_else(|| format!("Invalid timestamp: {}", self.timestamp));
        println!("Timestamp: {}\n", timestamp_str);

        if self.balances.is_empty() {
            println!("No balances found");
        } else {
            println!("{:<10} {:>20} {:>20} {:>20}", "Token", "Total", "Hold", "Available");
            println!("{}", "-".repeat(75));

            for balance in &self.balances {
                println!(
                    "{:<10} {:>20} {:>20} {:>20}",
                    balance.coin,
                    balance.total,
                    balance.hold,
                    balance.available.as_ref().unwrap_or(&"N/A".to_string())
                );
            }
        }
    }
}

// ===== USAGE EXAMPLE =====

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_balances() -> Result<()> {
        // Example usage
        let fetcher = HyperliquidSpotBalances::new()?;
        let user_address = "0xd36FE1BcDf2deEC384D1538360091eCf4c4a1688"; // Replace with actual address

        // Get all balances
        match fetcher.get_spot_balances(user_address).await {
            Ok(balances) => {
                for balance in balances {
                    println!("{}: {} (hold: {}, available: {:?})", balance.coin, balance.total, balance.hold, balance.available);
                }
            }
            Err(e) => eprintln!("Error fetching balances: {}", e),
        }

        // Get specific tokens
        match fetcher.get_main_balances(user_address).await {
            Ok(balances) => {
                println!("\nMain token balances:");
                for balance in balances {
                    println!("{}: {}", balance.coin, balance.total);
                }
            }
            Err(e) => eprintln!("Error fetching main balances: {}", e),
        }
        
        Ok(())
    }
}
