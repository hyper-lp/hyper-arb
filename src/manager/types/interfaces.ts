/**
 * Types for HyperLiquid precompile data structures
 * Based on hyper-evm-lib PrecompileLib.sol structs
 */

export interface SpotBalance {
    total: bigint;
    hold: bigint;
    entryNtl: bigint;
}

export interface TokenInfo {
    name: string;
    spots: bigint[];
    deployerTradingFeeShare: bigint;
    deployer: string;
    evmContract: string;
    szDecimals: number;
    weiDecimals: number;
    evmExtraWeiDecimals: number;
}

export interface SpotInfo {
    name: string;
    tokens: [bigint, bigint]; // [baseToken, quoteToken]
}

export interface AssetBalance {
    address: string;
    symbol: string;
    balance: bigint;
    decimals: number;
    priceUsd: bigint; // Price in USD with 8 decimals (hyper-evm-lib format)
    valueUsd: bigint; // Total value in USD
}

export interface Portfolio {
    assets: AssetBalance[];
    totalValueUsd: bigint;
    timestamp: number;
}

export interface RebalanceConfig {
    tokenA: string;
    tokenB: string;
    targetAllocation: number; // e.g., 50 for 50%
    thresholdBps: number; // deviation threshold in basis points
    enabled: boolean;
}

export interface RebalanceAnalysis {
    currentAllocationA: number; // percentage
    currentAllocationB: number; // percentage  
    deviationBps: number;
    needsRebalance: boolean;
    amountToSell?: bigint;
    tokenToSell?: string;
    amountToBuy?: bigint;
    tokenToBuy?: string;
}

// Configuration types for monitoring targets
export interface TargetConfig {
    vault_name: string;
    address: string;
    base_token: string;
    base_token_address: string;
    quote_token: string;
    quote_token_address: string;
    disabled_arb_treshold: number; // Threshold percentage for rebalancing trigger
    min_trade_value_usd: number;
    reference: string; // "pyth" | "hypercore" | "redstone"
    statistical_arb: boolean;
}

export interface GlobalConfig {
    network_name: string;
    rpc_endpoint: string;
    broadcast_rpc_endpoint: string;
    websocket_endpoint: string;
    hyperliquid_api_endpoint: string;
    explorer_base_url: string;
}

export interface HyperEVMConfig {
    core_bridge_contract: string;
    wrapped_hype_token_address: string;
    bridge_hype_token_address: string;
    liqd_multi_hop_router_address: string;
    liquidswap_api_endpoint: string;
}

export interface GasConfig {
    gas_estimate_multiplier: number;
    slippage_tolerance_percent: number;
    native_hype_reserve_amount: number;
    max_gas_price_gwei: number;
    gas_price_multiplier: number;
}

// Balance monitoring types
export interface WalletBalance {
    evmBalance: bigint;
    coreBalance: bigint;
    totalBalance: bigint;
    decimals: number;
}

export interface TokenBalance {
    address: string;
    symbol: string;
    balance: WalletBalance;
    priceUsd: bigint; // Price in USD with 8 decimals
    valueUsd: bigint; // Total value in USD with 8 decimals
}

export interface PortfolioSnapshot {
    baseToken: TokenBalance;
    quoteToken: TokenBalance;
    totalValueUsd: bigint;
    baseAllocationPercent: number;
    quoteAllocationPercent: number;
    timestamp: number;
}

export interface RebalanceDecision {
    needsRebalance: boolean;
    currentBaseAllocation: number;
    currentQuoteAllocation: number;
    threshold: number;
    amountToRebalance?: bigint;
    tokenToSell?: string;
    tokenToBuy?: string; // For statistical_arb: target token; for simple: bridge direction
    expectedValueUsd?: bigint;
    reason?: string;
    additionalData?: {
        // Legacy fields for backward compatibility
        baseBridgeAmount?: bigint;
        quoteBridgeAmount?: bigint;
        // New fields for complete bridge directions
        baseBridgeToCore?: bigint;
        baseBridgeToEvm?: bigint;
        quoteBridgeToCore?: bigint;
        quoteBridgeToEvm?: bigint;
        baseToken?: string;
        quoteToken?: string;
    };
}

export interface RebalanceExecutionResult {
    success: boolean;
    hash?: string;
    error?: string;
    decision?: RebalanceDecision;
    gasPrice?: number;
    whypeUnwrapped?: boolean;
    strategyUsed?: 'statistical-arb' | 'simple-dual-bridge' | 'none';
}
