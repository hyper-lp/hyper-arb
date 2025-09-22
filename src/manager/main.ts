/**
 * HyperArb Manager Service - TypeScript Implementation
 * Monitors portfolio allocations and rebalances when thresholds are exceeded
 */

import { ethers } from 'ethers';
import { readFileSync } from 'fs';
import * as TOML from 'toml';
import { BalanceMonitor } from './utils/BalanceMonitor.js';

// Multi-wallet environment configuration (matching Rust EnvConfig)
interface WalletConfig {
    publicKeys: string[];
    privateKeys: string[];
}

function loadWalletConfig(): WalletConfig {
    const pubKeysStr = process.env.WALLET_PUB_KEYS;
    const privateKeysStr = process.env.WALLET_PRIVATE_KEYS;
    
    // Demo mode: no private keys needed
    const demoMode = process.env.DEMO_MODE === 'true' || !pubKeysStr || !privateKeysStr;
    
    if (demoMode) {
        console.log('🎯 DEMO MODE: Running without private keys (read-only monitoring)');
        return { publicKeys: [], privateKeys: [] };
    }
    
    const publicKeys = pubKeysStr.split(',').map(s => s.trim().toLowerCase());
    const privateKeys = privateKeysStr.split(',').map(s => s.trim());
    
    if (publicKeys.length !== privateKeys.length) {
        throw new Error(`Wallet count mismatch: ${publicKeys.length} public keys vs ${privateKeys.length} private keys`);
    }
    
    console.log(`🔑 Loaded ${publicKeys.length} wallets`);
    return { publicKeys, privateKeys };
}

function getPrivateKeyForAddress(walletConfig: WalletConfig, targetAddress: string): string | null {
    const addressLower = targetAddress.toLowerCase();
    
    for (let i = 0; i < walletConfig.publicKeys.length; i++) {
        if (walletConfig.publicKeys[i] === addressLower) {
            return walletConfig.privateKeys[i];
        }
    }
    
    return null;
}

// Load config from TOML with target selection
function loadConfig() {
    const configPath = process.env.CONFIG_PATH || 'config/main.toml';
    const targetName = process.env.TARGET_NAME;
    
    console.log(`📂 Loading config from: ${configPath}`);
    if (targetName) {
        console.log(`🎯 Target filter: ${targetName}`);
    } else {
        console.log(`📊 Monitoring all targets`);
    }
    
    const configContent = readFileSync(configPath, 'utf-8');
    const config = TOML.parse(configContent);
    
    // Check if we're in demo mode first
    const pubKeysStr = process.env.WALLET_PUB_KEYS;
    const privateKeysStr = process.env.WALLET_PRIVATE_KEYS;
    const demoMode = process.env.DEMO_MODE === 'true' || !pubKeysStr || !privateKeysStr;
    
    if (demoMode) {
        console.log('🎯 DEMO MODE: Running without private keys (read-only monitoring)');
    }
    
    // Filter targets if TARGET_NAME is specified
    let targets = config.targets;
    if (targetName) {
        targets = config.targets.filter((t: any) => t.vault_name === targetName);
        if (targets.length === 0) {
            const availableTargets = config.targets.map((t: any) => t.vault_name).join(', ');
            console.log('❌ Available targets:', availableTargets);
            throw new Error(`Target '${targetName}' not found in config`);
        }
    }
    
    return {
        global: config.global,
        targets: targets,
        demoMode: demoMode
    };
}

class HyperArbManager {
    private config: any;
    private walletConfig: WalletConfig;
    private provider: ethers.JsonRpcProvider;
    private isRunning: boolean = false;
    private demoMode: boolean;
    
    constructor() {
        this.config = loadConfig();
        this.demoMode = this.config.demoMode;
        this.walletConfig = this.demoMode ? { publicKeys: [], privateKeys: [] } : loadWalletConfig();
        this.provider = new ethers.JsonRpcProvider(this.config.global.rpc_endpoint);
        
        // Setup graceful shutdown
        this.setupGracefulShutdown();
    }
    
    async start() {
        console.log('🚀 HyperArb Manager starting...');
        
        if (this.demoMode) {
            console.log('🎯 DEMO MODE: Read-only monitoring (no transactions will be sent)');
            console.log('📊 Monitoring all targets for rebalancing opportunities...');
        } else {
            console.log(`🔑 Multi-wallet system initialized with ${this.walletConfig.publicKeys.length} wallets`);
        }
        
        console.log(`📊 Configured ${this.config.targets.length} targets:`);
        
        // Log target configurations
        for (const target of this.config.targets) {
            console.log(`• ${target.vault_name} (${target.base_token}/${target.quote_token}) - Address: ${target.address.slice(0, 10)}...`);
            console.log(`  ⚖️ Threshold: ${target.disabled_arb_treshold}% | Mode: ${target.statistical_arb ? 'Statistical Arb' : 'Simple Bridge'}`);
        }
        
        console.log(`🌐 RPC: ${this.config.global.rpc_endpoint}`);
        
        this.isRunning = true;
        
        while (this.isRunning) {
            try {
                // Process each target (matching arbitrager pattern)
                for (const target of this.config.targets) {
                    if (!this.isRunning) break;
                    
                    console.log(`\\n📊 Checking ${target.vault_name} target... [${new Date().toLocaleTimeString()}]`);
                    
                    if (this.demoMode) {
                        // Demo mode: monitor without private keys
                        await this.demoModeMonitoring(target);
                    } else {
                        // Production mode: full monitoring with transactions
                        await this.productionModeMonitoring(target);
                    }
                    
                    // Brief pause between targets
                    if (this.config.targets.length > 1) {
                        await this.sleep(100);
                    }
                }
                
            } catch (error: any) {
                console.error('💥 Error:', error.message);
                // Continue running unless it's a fatal error
                if (error.message.includes('WALLET_') || error.message.includes('config')) {
                    console.error('🚨 Fatal error detected, shutting down...');
                    break;
                }
            }
            
            // Wait 0.25 seconds before next check cycle
            await this.sleep(250);
        }
        
        console.log('👋 Manager stopped');
    }
    
    private sleep(ms: number): Promise<void> {
        return new Promise(resolve => setTimeout(resolve, ms));
    }
    
    private setupGracefulShutdown(): void {
        const signals = ['SIGTERM', 'SIGINT', 'SIGUSR2'];
        
        signals.forEach(signal => {
            process.on(signal, () => {
                console.log(`\\n🛑 Received ${signal}, shutting down gracefully...`);
                this.isRunning = false;
            });
        });
    }

    private async demoModeMonitoring(target: any) {
        console.log(`🎯 [${target.vault_name}] DEMO: Monitoring for rebalancing...`);
        
        try {
            // Create a dummy signer for read-only operations (won't be used for transactions)
            const dummySigner = new ethers.Wallet('0x' + '1'.repeat(64), this.provider);
            const { BalanceMonitor } = await import('./utils/BalanceMonitor.js');
            const monitor = new BalanceMonitor(this.provider, dummySigner);
            
            // Get real portfolio snapshot
            const portfolio = await monitor.getPortfolioSnapshot(target);
            
            console.log(`📊 Portfolio Snapshot:`);
            console.log(`  Base (${target.base_token}): ${ethers.formatUnits(portfolio.baseToken.balance.totalBalance, portfolio.baseToken.balance.decimals)} ($${Number(portfolio.baseToken.valueUsd) / 1e8})`);
            console.log(`    EVM: ${ethers.formatUnits(portfolio.baseToken.balance.evmBalance, portfolio.baseToken.balance.decimals)}`);
            console.log(`    Core: ${ethers.formatUnits(portfolio.baseToken.balance.coreBalance, portfolio.baseToken.balance.decimals)}`);
            console.log(`  Quote (${target.quote_token}): ${ethers.formatUnits(portfolio.quoteToken.balance.totalBalance, portfolio.quoteToken.balance.decimals)} ($${Number(portfolio.quoteToken.valueUsd) / 1e8})`);
            console.log(`    EVM: ${ethers.formatUnits(portfolio.quoteToken.balance.evmBalance, portfolio.quoteToken.balance.decimals)}`);
            console.log(`    Core: ${ethers.formatUnits(portfolio.quoteToken.balance.coreBalance, portfolio.quoteToken.balance.decimals)}`);
            console.log(`  Total Value: $${Number(portfolio.totalValueUsd) / 1e8}`);
            console.log(`  Allocations: ${portfolio.baseAllocationPercent.toFixed(2)}% Base / ${portfolio.quoteAllocationPercent.toFixed(2)}% Quote`);
            
            // Analyze rebalancing need
            const decision = await monitor.analyzeRebalanceNeed(target);
            
            if (decision.needsRebalance) {
                console.log(`⚠️  REBALANCING NEEDED: ${decision.reason}`);
                console.log(`  ${target.statistical_arb ? 'Statistical Arb' : 'Simple Bridge'} strategy would:`);
                if (decision.tokenToSell && decision.tokenToBuy && decision.amountToRebalance) {
                    console.log(`  - Sell: ${decision.amountToRebalance} of ${decision.tokenToSell}`);
                    console.log(`  - Buy: ${decision.tokenToBuy}`);
                    console.log(`  - Value: $${Number(decision.expectedValueUsd || 0n) / 1e8}`);
                }
                
                // Show detailed transaction steps that would be executed
                await this.simulateRebalancingSteps(target, decision, monitor);
                
                console.log(`🚫 [DEMO] Would execute rebalancing but transactions disabled`);
            } else {
                console.log(`✅ No rebalancing needed: ${decision.reason}`);
            }
            
        } catch (error: any) {
            console.error(`❌ [DEMO] Error during monitoring: ${error.message}`);
        }
    }

    private async simulateRebalancingSteps(target: any, decision: any, monitor: any) {
        console.log(`\n🔍 DETAILED TRANSACTION SIMULATION:`);
        
        try {
            if (target.statistical_arb) {
                await this.simulateStatisticalArbSteps(target, decision, monitor);
            } else {
                await this.simulateSimpleBridgeSteps(target, decision, monitor);
            }
        } catch (error: any) {
            console.log(`❌ Simulation error: ${error.message}`);
        }
    }
    
    private async simulateStatisticalArbSteps(target: any, decision: any, monitor: any) {
        console.log(`📈 Statistical Arbitrage Flow:`);
        
        if (!decision.tokenToSell || !decision.tokenToBuy || !decision.amountToRebalance) {
            console.log(`❌ Missing rebalancing parameters`);
            return;
        }
        
        const tokenToSellAmount = decision.amountToRebalance;
        const tokenToSellSymbol = decision.tokenToSell === target.base_token_address ? target.base_token : target.quote_token;
        const tokenToBuySymbol = decision.tokenToBuy === target.base_token_address ? target.base_token : target.quote_token;
        
        // Check if we need WHYPE unwrapping first
        if (decision.tokenToSell === '0x5555555555555555555555555555555555555555') {
            console.log(`\n  Step 1a: Unwrap ${ethers.formatEther(tokenToSellAmount)} WHYPE → HYPE for bridging`);
            console.log(`    📤 Would call: unwrapWhype(${tokenToSellAmount})`);
            console.log(`    💡 Always unwrap WHYPE before bridging to Core`);
            
            console.log(`\n  Step 1b: Bridge ${ethers.formatEther(tokenToSellAmount)} HYPE from EVM → HyperCore`);
            console.log(`    📤 Would call: bridgeToCore({ token: 0x2222222222222222222222222222222222222222, amount: ${tokenToSellAmount} })`);
        } else {
            console.log(`\n  Step 1: Bridge ${ethers.formatEther(tokenToSellAmount)} ${tokenToSellSymbol} from EVM → HyperCore`);
            console.log(`    📤 Would call: bridgeToCore({ token: ${decision.tokenToSell}, amount: ${tokenToSellAmount} })`);
        }
        
        console.log(`\n  Step 2: Execute swap on HyperCore`);
        if (decision.tokenToSell === target.quote_token_address) {
            console.log(`    📤 Would call: swapUsdcToAsset({ token: ${decision.tokenToBuy}, amount: ${tokenToSellAmount} })`);
            console.log(`    💱 Swapping ${tokenToSellSymbol} → ${tokenToBuySymbol} via USDC intermediate`);
        } else {
            console.log(`    📤 Would call: swapAssetToUsdc({ token: ${decision.tokenToSell}, amount: ${tokenToSellAmount} })`);
            console.log(`    💱 Swapping ${tokenToSellSymbol} → ${tokenToBuySymbol} via USDC intermediate`);
        }
        
        console.log(`\n  Step 3: Wait for swap completion and bridge back`);
        console.log(`    📤 Would call: checkSwapCompleted(${decision.tokenToBuy}, expectedAmount)`);
        console.log(`    📤 Would call: bridgeToEvm({ token: ${decision.tokenToBuy}, amount: receivedAmount })`);
        console.log(`    🏠 Bridge ${tokenToBuySymbol} from HyperCore → EVM`);
        
        // Check if we need HYPE wrapping at the end (when buying HYPE for wHYPE target)
        if (decision.tokenToBuy === '0x2222222222222222222222222222222222222222' && 
            target.base_token === 'wHYPE') {
            console.log(`\n  Step 4: Wrap exactly received HYPE amount to WHYPE`);
            console.log(`    🔄 Would wrap the exact received HYPE → WHYPE for trading consistency`);
            console.log(`    📤 Would call: wrapWhype(receivedAmount)`);
            console.log(`    💡 Always wrap received HYPE when target is wHYPE`);
        }
        
        const estimatedValueUsd = Number(decision.expectedValueUsd || 0n) / 1e8;
        console.log(`\n  💰 Total transaction value: ~$${estimatedValueUsd.toFixed(2)}`);
        console.log(`  🎯 Goal: Achieve 50-50 allocation on EVM layer`);
    }
    
    private async simulateSimpleBridgeSteps(target: any, decision: any, monitor: any) {
        console.log(`🌉 Simple Dual-Bridge Flow:`);
        
        if (!decision.additionalData) {
            console.log(`❌ Missing bridge amounts data`);
            return;
        }
        
        const {
            baseBridgeToCore = 0n,
            baseBridgeToEvm = 0n,
            quoteBridgeToCore = 0n,
            quoteBridgeToEvm = 0n
        } = decision.additionalData;
        
        let stepNum = 1;
        
        if (baseBridgeToCore > 0n) {
            console.log(`\n  Step ${stepNum++}: Bridge ${ethers.formatEther(baseBridgeToCore)} ${target.base_token} EVM → Core`);
            console.log(`    📤 Would call: bridgeToCore({ token: ${target.base_token_address}, amount: ${baseBridgeToCore} })`);
        }
        
        if (baseBridgeToEvm > 0n) {
            console.log(`\n  Step ${stepNum++}: Bridge ${ethers.formatEther(baseBridgeToEvm)} ${target.base_token} Core → EVM`);
            console.log(`    📤 Would call: bridgeToEvm({ token: ${target.base_token_address}, amount: ${baseBridgeToEvm} })`);
        }
        
        if (quoteBridgeToCore > 0n) {
            console.log(`\n  Step ${stepNum++}: Bridge ${ethers.formatEther(quoteBridgeToCore)} ${target.quote_token} EVM → Core`);
            console.log(`    📤 Would call: bridgeToCore({ token: ${target.quote_token_address}, amount: ${quoteBridgeToCore} })`);
        }
        
        if (quoteBridgeToEvm > 0n) {
            console.log(`\n  Step ${stepNum++}: Bridge ${ethers.formatEther(quoteBridgeToEvm)} ${target.quote_token} Core → EVM`);
            console.log(`    📤 Would call: bridgeToEvm({ token: ${target.quote_token_address}, amount: ${quoteBridgeToEvm} })`);
        }
        
        const estimatedValueUsd = Number(decision.expectedValueUsd || 0n) / 1e8;
        console.log(`\n  💰 Total bridge value: ~$${estimatedValueUsd.toFixed(2)}`);
        console.log(`  🎯 Goal: Mirror 50-50 allocation on both EVM and Core layers`);
    }

    private async productionModeMonitoring(target: any) {
        // Production mode logic (existing code would go here)
        console.log(`🔑 [${target.vault_name}] Production monitoring not yet implemented`);
    }
}

// Entry point
async function main() {
    try {
        const manager = new HyperArbManager();
        await manager.start();
    } catch (error: any) {
        console.error('❌ Failed to start manager service:', error.message);
        process.exit(1);
    }
}

// Start if this is the main module
if (import.meta.url === `file://${process.argv[1]}`) {
    main();
}