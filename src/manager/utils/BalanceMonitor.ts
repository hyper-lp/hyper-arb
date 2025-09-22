/**
 * BalanceMonitor for wallet balance tracking and rebalancing calculations
 * Monitors portfolios across EVM and HyperCore for rebalancing decisions
 */

import { ethers } from 'ethers';
import { PrecompileUtils } from './PrecompileUtils.js';
import {
    TargetConfig,
    PortfolioSnapshot,
    TokenBalance,
    WalletBalance,
    RebalanceDecision
} from '../types/interfaces.js';

// WHYPE contract ABI for wrapping/unwrapping operations
const WHYPE_ABI = [
    "function withdraw(uint256 wad) external",
    "function deposit() external payable",
    "function balanceOf(address owner) external view returns (uint256)",
    "function allowance(address owner, address spender) external view returns (uint256)",
    "function approve(address spender, uint256 amount) external returns (bool)"
];

// ERC20 ABI for token operations
const ERC20_ABI = [
    "function balanceOf(address owner) external view returns (uint256)",
    "function decimals() external view returns (uint8)",
    "function symbol() external view returns (string)",
    "function transfer(address to, uint256 amount) external returns (bool)"
];

export class BalanceMonitor {
    private provider: ethers.Provider;
    private signer: ethers.Signer;
    private precompileUtils: PrecompileUtils;
    private whypeContract: ethers.Contract;

    constructor(provider: ethers.Provider, signer: ethers.Signer) {
        this.provider = provider;
        this.signer = signer;
        this.precompileUtils = new PrecompileUtils(provider);
        this.whypeContract = new ethers.Contract(
            '0x5555555555555555555555555555555555555555',
            WHYPE_ABI,
            signer
        );
    }

    /**
     * Get complete portfolio snapshot for a target configuration
     * Handles statistical_arb vs non-statistical modes differently
     */
    async getPortfolioSnapshot(target: TargetConfig): Promise<PortfolioSnapshot> {
        const [baseTokenBalance, quoteTokenBalance] = await Promise.all([
            this.getTokenBalance(target.address, target.base_token_address, target.base_token, target.statistical_arb),
            this.getTokenBalance(target.address, target.quote_token_address, target.quote_token, target.statistical_arb)
        ]);

        const totalValueUsd = baseTokenBalance.valueUsd + quoteTokenBalance.valueUsd;

        const baseAllocationPercent = totalValueUsd > 0n
            ? Number((baseTokenBalance.valueUsd * 10000n) / totalValueUsd) / 100
            : 0;

        const quoteAllocationPercent = 100 - baseAllocationPercent;

        return {
            baseToken: baseTokenBalance,
            quoteToken: quoteTokenBalance,
            totalValueUsd,
            baseAllocationPercent,
            quoteAllocationPercent,
            timestamp: Math.floor(Date.now() / 1000)
        };
    }

    /**
     * Get comprehensive token balance across EVM and HyperCore
     * @param statisticalArb If true, only use EVM balance; if false, use both EVM + Core
     */
    async getTokenBalance(walletAddress: string, tokenAddress: string, symbol: string, statisticalArb: boolean = false): Promise<TokenBalance> {
        const [evmBalance, coreBalance, priceUsd, decimals] = await Promise.all([
            this.getEvmBalance(walletAddress, tokenAddress, statisticalArb),
            statisticalArb ? Promise.resolve(0n) : this.getCoreBalance(walletAddress, tokenAddress),
            this.getTokenPriceUsd(tokenAddress),
            this.getTokenDecimals(tokenAddress)
        ]);

        const totalBalance = evmBalance + coreBalance;
        // Price is in 8 decimals from HyperCore, totalBalance is in EVM decimals
        // Result should be in 8-decimal USD format to match HyperCore precision
        const valueUsd = (totalBalance * priceUsd) / (10n ** BigInt(decimals));

        return {
            address: tokenAddress,
            symbol,
            balance: {
                evmBalance,
                coreBalance,
                totalBalance,
                decimals
            },
            priceUsd,
            valueUsd
        };
    }

    /**
     * Get EVM balance for a token
     * @param statisticalArb If true and token is WHYPE, only return WHYPE balance (not native HYPE)
     */
    async getEvmBalance(walletAddress: string, tokenAddress: string, statisticalArb: boolean = false): Promise<bigint> {
        try {
            // Handle WHYPE token - in statistical arb mode, only count WHYPE balance
            if (tokenAddress === '0x5555555555555555555555555555555555555555') {
                const whypeContract = new ethers.Contract(tokenAddress, ERC20_ABI, this.provider);
                return await whypeContract.balanceOf(walletAddress);
            }

            // Handle native HYPE balance - only used for non-statistical arb or gas calculations
            if (tokenAddress === '0x2222222222222222222222222222222222222222') {
                return await this.provider.getBalance(walletAddress);
            }

            // Handle other ERC20 tokens
            const tokenContract = new ethers.Contract(tokenAddress, ERC20_ABI, this.provider);
            return await tokenContract.balanceOf(walletAddress);
        } catch (error) {
            console.warn(`Failed to get EVM balance for ${tokenAddress}: ${error}`);
            return 0n;
        }
    }

    /**
     * Get HyperCore balance for a token
     */
    async getCoreBalance(walletAddress: string, tokenAddress: string): Promise<bigint> {
        try {
            const balance = await this.precompileUtils.getSpotBalance(walletAddress, tokenAddress);
            // Convert core wei to EVM format for consistent handling
            return await this.precompileUtils.weiToEvm(tokenAddress, balance.total);
        } catch (error) {
            console.warn(`Failed to get Core balance for ${tokenAddress}: ${error}`);
            return 0n;
        }
    }

    /**
     * Get token price in USD with 8 decimals using precompiles
     */
    async getTokenPriceUsd(tokenAddress: string): Promise<bigint> {
        try {
            return await this.precompileUtils.getSpotPrice(tokenAddress);
        } catch (error) {
            console.warn(`Failed to get price for ${tokenAddress}: ${error}`);
            return 0n;
        }
    }

    /**
     * Get token decimals
     */
    async getTokenDecimals(tokenAddress: string): Promise<number> {
        try {
            // Handle native HYPE (18 decimals)
            if (tokenAddress === '0x2222222222222222222222222222222222222222') {
                return 18;
            }

            const tokenContract = new ethers.Contract(tokenAddress, ERC20_ABI, this.provider);
            return await tokenContract.decimals();
        } catch (error) {
            // Default to 18 decimals if we can't determine
            return 18;
        }
    }

    /**
     * Determine if rebalancing is needed based on target configuration
     * High-level function that routes to appropriate rebalancing strategy
     */
    async analyzeRebalanceNeed(target: TargetConfig): Promise<RebalanceDecision> {
        if (target.statistical_arb) {
            return await this.analyzeStatisticalArbRebalance(target);
        } else {
            return await this.analyzeSimpleRebalance(target);
        }
    }

    /**
     * Analyze rebalancing need for statistical arbitrage mode
     * Only considers EVM balances, rebalancing via HyperCore swaps
     */
    private async analyzeStatisticalArbRebalance(target: TargetConfig): Promise<RebalanceDecision> {
        const portfolio = await this.getPortfolioSnapshot(target);

        // Guard against zero portfolio value
        if (portfolio.totalValueUsd === 0n) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold: target.disabled_arb_treshold,
                reason: 'Zero portfolio value - no assets to rebalance'
            };
        }

        // Check if either allocation is below the threshold percentage
        const threshold = target.disabled_arb_treshold;
        const needsRebalance =
            portfolio.baseAllocationPercent < threshold ||
            portfolio.quoteAllocationPercent < threshold;

        if (!needsRebalance) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold,
                reason: 'Statistical arb mode - allocations within threshold'
            };
        }

        // Determine rebalancing strategy for statistical arbitrage
        let tokenToSell: string;
        let tokenToBuy: string;
        let amountToRebalance: bigint;

        if (portfolio.baseAllocationPercent > portfolio.quoteAllocationPercent) {
            tokenToSell = target.base_token_address;
            tokenToBuy = target.quote_token_address;

            // Calculate amount needed to reach 50-50 split
            // Example: 82/18 ratio → excess = 82% - 50% = 32% of total portfolio
            // This 32% represents ~39% of the overallocated asset (32% / 82% ≈ 39%)
            const targetBaseValue = portfolio.totalValueUsd / 2n;
            const excessBaseValue = portfolio.baseToken.valueUsd - targetBaseValue;

            const basePrice = portfolio.baseToken.priceUsd;
            // excessBaseValue is 8-decimal USD, basePrice is 8-decimal, need token amount in EVM decimals  
            amountToRebalance = (excessBaseValue * (10n ** BigInt(portfolio.baseToken.balance.decimals))) / basePrice;
        } else {
            tokenToSell = target.quote_token_address;
            tokenToBuy = target.base_token_address;

            // Calculate excess quote token amount (same logic as above)
            const targetQuoteValue = portfolio.totalValueUsd / 2n;
            const excessQuoteValue = portfolio.quoteToken.valueUsd - targetQuoteValue;

            const quotePrice = portfolio.quoteToken.priceUsd;
            // excessQuoteValue is 8-decimal USD, quotePrice is 8-decimal, need token amount in EVM decimals
            amountToRebalance = (excessQuoteValue * (10n ** BigInt(portfolio.quoteToken.balance.decimals))) / quotePrice;
        }

        // Validate minimum trade value and available balance
        const tokenToSellBalance = tokenToSell === target.base_token_address
            ? portfolio.baseToken.balance.totalBalance
            : portfolio.quoteToken.balance.totalBalance;

        if (amountToRebalance > tokenToSellBalance) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold,
                reason: `Insufficient balance for rebalancing (need ${amountToRebalance}, have ${tokenToSellBalance})`
            };
        }

        // Calculate USD value: token_amount_EVM_decimals * price_8_decimals / 10^EVM_decimals = value_8_decimal_USD
        const tokenPrice = tokenToSell === target.base_token_address ? portfolio.baseToken.priceUsd : portfolio.quoteToken.priceUsd;
        const tokenDecimals = tokenToSell === target.base_token_address ? portfolio.baseToken.balance.decimals : portfolio.quoteToken.balance.decimals;
        const rebalanceValueUsd = (amountToRebalance * tokenPrice) / (10n ** BigInt(tokenDecimals));
        const minTradeValueUsd = BigInt(Math.floor(target.min_trade_value_usd * 1e8));

        if (rebalanceValueUsd < minTradeValueUsd) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold,
                reason: `Rebalance amount ($${Number(rebalanceValueUsd) / 1e8}) below minimum ($${target.min_trade_value_usd})`
            };
        }

        return {
            needsRebalance: true,
            currentBaseAllocation: portfolio.baseAllocationPercent,
            currentQuoteAllocation: portfolio.quoteAllocationPercent,
            threshold,
            amountToRebalance,
            tokenToSell,
            tokenToBuy,
            expectedValueUsd: rebalanceValueUsd,
            reason: `Statistical arb: ${portfolio.baseAllocationPercent < threshold ? 'Base' : 'Quote'} allocation below ${threshold}% threshold`
        };
    }

    /**
     * Analyze rebalancing need for simple dual-bridge mode  
     * Monitors both EVM and Core balances, rebalancing via dual bridging to mirror 50-50 on both layers
     */
    private async analyzeSimpleRebalance(target: TargetConfig): Promise<RebalanceDecision> {
        const portfolio = await this.getPortfolioSnapshot(target);

        // Guard against zero portfolio value
        if (portfolio.totalValueUsd === 0n) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold: target.disabled_arb_treshold,
                reason: 'Zero portfolio value - no assets to rebalance'
            };
        }

        // Check if either allocation is below the threshold percentage
        const threshold = target.disabled_arb_treshold;
        const needsRebalance =
            portfolio.baseAllocationPercent < threshold ||
            portfolio.quoteAllocationPercent < threshold;

        if (!needsRebalance) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold,
                reason: 'Simple mode - allocations within threshold'
            };
        }

        // Simple mode: Calculate dual bridging to make both layers mirrors (50-50 each)
        // Example: EVM 80% A, 20% B and Core 20% A, 80% B
        //          → Bridge 30% A from EVM→Core and 30% B from Core→EVM
        const baseEvmBalance = portfolio.baseToken.balance.evmBalance;
        const baseCoreBalance = portfolio.baseToken.balance.coreBalance;
        const quoteEvmBalance = portfolio.quoteToken.balance.evmBalance;
        const quoteCoreBalance = portfolio.quoteToken.balance.coreBalance;

        // Calculate target 50-50 balance for each layer
        const totalBaseTokens = baseEvmBalance + baseCoreBalance;
        const totalQuoteTokens = quoteEvmBalance + quoteCoreBalance;
        const targetBasePerLayer = totalBaseTokens / 2n;
        const targetQuotePerLayer = totalQuoteTokens / 2n;

        // Calculate bridge amounts needed for each asset (all 4 directions)
        const baseBridgeToCore = baseEvmBalance > targetBasePerLayer
            ? baseEvmBalance - targetBasePerLayer // Bridge base from EVM → Core
            : 0n;
        const baseBridgeToEvm = baseCoreBalance > targetBasePerLayer
            ? baseCoreBalance - targetBasePerLayer // Bridge base from Core → EVM
            : 0n;
        const quoteBridgeToCore = quoteEvmBalance > targetQuotePerLayer
            ? quoteEvmBalance - targetQuotePerLayer // Bridge quote from EVM → Core
            : 0n;
        const quoteBridgeToEvm = quoteCoreBalance > targetQuotePerLayer
            ? quoteCoreBalance - targetQuotePerLayer // Bridge quote from Core → EVM
            : 0n;

        // Calculate USD values for all bridge operations
        const baseToCoreValue = (baseBridgeToCore * portfolio.baseToken.priceUsd) / (10n ** BigInt(portfolio.baseToken.balance.decimals));
        const baseToEvmValue = (baseBridgeToEvm * portfolio.baseToken.priceUsd) / (10n ** BigInt(portfolio.baseToken.balance.decimals));
        const quoteToCoreValue = (quoteBridgeToCore * portfolio.quoteToken.priceUsd) / (10n ** BigInt(portfolio.quoteToken.balance.decimals));
        const quoteToEvmValue = (quoteBridgeToEvm * portfolio.quoteToken.priceUsd) / (10n ** BigInt(portfolio.quoteToken.balance.decimals));

        const totalBridgeValue = baseToCoreValue + baseToEvmValue + quoteToCoreValue + quoteToEvmValue;

        // Validate minimum trade value for total bridge operations
        const primaryValueUsd = totalBridgeValue;
        const minTradeValueUsd = BigInt(Math.floor(target.min_trade_value_usd * 1e8));

        if (primaryValueUsd < minTradeValueUsd) {
            return {
                needsRebalance: false,
                currentBaseAllocation: portfolio.baseAllocationPercent,
                currentQuoteAllocation: portfolio.quoteAllocationPercent,
                threshold,
                reason: `Simple mode: Bridge amounts below minimum ($${Number(primaryValueUsd) / 1e8} < $${target.min_trade_value_usd})`
            };
        }

        return {
            needsRebalance: true,
            currentBaseAllocation: portfolio.baseAllocationPercent,
            currentQuoteAllocation: portfolio.quoteAllocationPercent,
            threshold,
            amountToRebalance: totalBridgeValue, // Total USD value being bridged
            tokenToSell: 'multiple', // Multiple tokens/directions
            tokenToBuy: 'dual-bridge', // Direction indicator  
            expectedValueUsd: primaryValueUsd,
            reason: `Simple mode: Multi-directional bridge needed - Base(${ethers.formatUnits(baseBridgeToCore, portfolio.baseToken.balance.decimals)}→Core, ${ethers.formatUnits(baseBridgeToEvm, portfolio.baseToken.balance.decimals)}→EVM), Quote(${ethers.formatUnits(quoteBridgeToCore, portfolio.quoteToken.balance.decimals)}→Core, ${ethers.formatUnits(quoteBridgeToEvm, portfolio.quoteToken.balance.decimals)}→EVM)`,
            // Store ALL bridge amounts and directions for execution
            additionalData: {
                baseBridgeToCore,
                baseBridgeToEvm,
                quoteBridgeToCore,
                quoteBridgeToEvm,
                baseToken: target.base_token_address,
                quoteToken: target.quote_token_address
            }
        };
    }

    /**
     * Wrap native HYPE to WHYPE
     * @param amount Amount of native HYPE to wrap (in HYPE wei units)
     */
    async wrapWhype(amount: bigint): Promise<{ success: boolean; hash?: string; error?: string }> {
        try {
            const tx = await this.whypeContract.deposit({ value: amount });
            const receipt = await tx.wait();

            return {
                success: receipt.status === 1,
                hash: tx.hash
            };
        } catch (error) {
            return {
                success: false,
                error: (error as Error).message
            };
        }
    }

    /**
     * Unwrap WHYPE to native HYPE
     * @param amount Amount of WHYPE to unwrap (in WHYPE units)
     */
    async unwrapWhype(amount: bigint): Promise<{ success: boolean; hash?: string; error?: string }> {
        try {
            const tx = await this.whypeContract.withdraw(amount);
            const receipt = await tx.wait();

            return {
                success: receipt.status === 1,
                hash: tx.hash
            };
        } catch (error) {
            return {
                success: false,
                error: (error as Error).message
            };
        }
    }

    /**
     * Check if wallet should wrap HYPE to WHYPE for trading
     */
    async shouldWrapHype(targetWhypeAmount: bigint): Promise<{ shouldWrap: boolean; whypeBalance: bigint; hypeBalance: bigint; availableForWrapping: bigint }> {
        try {
            const walletAddress = await this.signer.getAddress();

            const [hypeBalance, whypeBalance] = await Promise.all([
                this.provider.getBalance(walletAddress),
                this.whypeContract.balanceOf(walletAddress)
            ]);

            // Keep some HYPE for gas (default 0.1 HYPE)
            const gasReserve = BigInt(Math.floor(0.1 * 1e18)); // 0.1 HYPE in wei
            const availableForWrapping = hypeBalance > gasReserve ? hypeBalance - gasReserve : 0n;

            const totalWhypeNeeded = targetWhypeAmount > whypeBalance ? targetWhypeAmount - whypeBalance : 0n;
            const shouldWrap = totalWhypeNeeded > 0n && availableForWrapping >= totalWhypeNeeded;

            return {
                shouldWrap,
                whypeBalance,
                hypeBalance,
                availableForWrapping
            };
        } catch (error) {
            return {
                shouldWrap: false,
                whypeBalance: 0n,
                hypeBalance: 0n,
                availableForWrapping: 0n
            };
        }
    }

    /**
     * Check if wallet needs WHYPE unwrapping for sufficient HYPE balance
     */
    async shouldUnwrapWhype(requiredHype: bigint): Promise<{ shouldUnwrap: boolean; whypeBalance: bigint; hypeBalance: bigint }> {
        try {
            const walletAddress = await this.signer.getAddress();

            const [hypeBalance, whypeBalance] = await Promise.all([
                this.provider.getBalance(walletAddress),
                this.whypeContract.balanceOf(walletAddress)
            ]);

            const shouldUnwrap = hypeBalance < requiredHype && whypeBalance > 0n;

            return {
                shouldUnwrap,
                whypeBalance,
                hypeBalance
            };
        } catch (error) {
            return {
                shouldUnwrap: false,
                whypeBalance: 0n,
                hypeBalance: 0n
            };
        }
    }

    /*//////////////////////////////////////////////////////////////
                        HIGH-LEVEL REBALANCING FUNCTIONS
    //////////////////////////////////////////////////////////////*/

    /**
     * Execute rebalancing based on target configuration and decision
     * Routes to appropriate execution strategy (statistical arb vs simple bridge)
     */
    async executeRebalancing(target: TargetConfig, decision: RebalanceDecision): Promise<{
        success: boolean;
        hash?: string;
        error?: string;
        strategyUsed?: string;
    }> {
        if (!decision.needsRebalance) {
            return {
                success: false,
                error: 'No rebalancing needed',
                strategyUsed: 'none'
            };
        }

        if (target.statistical_arb) {
            return await this.executeStatisticalArbRebalancing(target, decision);
        } else {
            return await this.executeSimpleRebalancing(target, decision);
        }
    }

    /**
     * Execute statistical arbitrage rebalancing using HyperCore swaps
     * Complete flow: EVM → HyperCore → Swap → HyperCore → EVM (to maintain 50-50 on EVM)
     */
    private async executeStatisticalArbRebalancing(target: TargetConfig, decision: RebalanceDecision): Promise<{
        success: boolean;
        hash?: string;
        error?: string;
        strategyUsed?: string;
    }> {
        try {
            if (!decision.tokenToSell || !decision.tokenToBuy || !decision.amountToRebalance) {
                return {
                    success: false,
                    error: 'Missing rebalancing parameters',
                    strategyUsed: 'statistical-arb'
                };
            }

            // Statistical arbitrage complete flow:
            // 1. Bridge excess tokens from EVM → HyperCore 
            // 2. Execute swap on HyperCore (using USDC intermediate)
            // 3. Bridge swapped results back HyperCore → EVM 
            // Goal: Achieve 50-50 balance on EVM

            const coreWriterUtils = await import('./CoreWriterUtils.js').then(m => m.CoreWriterUtils);
            const coreWriter = new coreWriterUtils(this.provider, this.signer);

            console.log(`Statistical arbitrage rebalancing:`);
            console.log(`  - Selling: ${decision.tokenToSell} (${decision.amountToRebalance} units)`);
            console.log(`  - Buying: ${decision.tokenToBuy}`);
            console.log(`  - Expected Value: $${Number(decision.expectedValueUsd || 0n) / 1e8}`);

            // Step 1: Handle WHYPE unwrapping if selling WHYPE, then bridge to HyperCore
            let actualTokenToBridge = decision.tokenToSell;
            let actualAmountToBridge = decision.amountToRebalance;

            if (decision.tokenToSell === '0x5555555555555555555555555555555555555555') {
                console.log(`Step 1a: Unwrapping ${decision.amountToRebalance} WHYPE to HYPE for bridging...`);

                const unwrapResult = await this.unwrapWhype(decision.amountToRebalance);
                if (!unwrapResult.success) {
                    return {
                        success: false,
                        error: `Failed to unwrap WHYPE: ${unwrapResult.error}`,
                        hash: unwrapResult.hash,
                        strategyUsed: 'statistical-arb'
                    };
                }

                // Now bridge the unwrapped HYPE (native HYPE address)
                actualTokenToBridge = '0x2222222222222222222222222222222222222222';
                actualAmountToBridge = decision.amountToRebalance; // Same amount, just unwrapped
                console.log(`   WHYPE unwrapped to HYPE: ${unwrapResult.hash}`);
            }

            console.log(`Step 1b: Bridging ${actualAmountToBridge} tokens to HyperCore...`);
            const bridgeToResult = await coreWriter.bridgeToCore({
                token: actualTokenToBridge,
                amount: actualAmountToBridge
            });

            if (!bridgeToResult.success) {
                return {
                    success: false,
                    error: `Bridge to core failed: ${bridgeToResult.error}`,
                    hash: bridgeToResult.hash,
                    strategyUsed: 'statistical-arb'
                };
            }

            // Step 2: Execute swap on HyperCore using USDC intermediate
            console.log(`Step 2: Executing swap on HyperCore...`);
            let swapResult;
            if (decision.tokenToSell === target.quote_token_address) {
                // Selling quote token (USDT) for base token
                swapResult = await coreWriter.swapUsdcToAsset({
                    token: decision.tokenToBuy,
                    amount: actualAmountToBridge
                });
            } else {
                // Selling base token for quote token (USDT)
                swapResult = await coreWriter.swapAssetToUsdc({
                    token: actualTokenToBridge, // Use the actual bridged token (HYPE if unwrapped from WHYPE)
                    amount: actualAmountToBridge
                });
            }

            if (!swapResult.success) {
                return {
                    success: false,
                    error: `HyperCore swap failed: ${swapResult.error}`,
                    hash: swapResult.hash,
                    strategyUsed: 'statistical-arb'
                };
            }

            // Step 3: Wait for swap completion, then bridge results back to EVM
            console.log(`Step 3: Checking swap completion...`);

            // Check if swap completed and get the actual received amount
            const swapStatus = await coreWriter.checkSwapCompleted(
                decision.tokenToBuy,
                swapResult.expectedAmount || 0n
            );

            if (!swapStatus.completed) {
                return {
                    success: false,
                    error: `Swap not completed. Received: ${swapStatus.actualAmount}, Expected: ${swapResult.expectedAmount}`,
                    hash: swapResult.hash,
                    strategyUsed: 'statistical-arb'
                };
            }

            // Step 4: Bridge the swapped tokens back from HyperCore → EVM
            const receivedAmount = swapStatus.actualAmount || swapResult.expectedAmount || 0n;
            console.log(`Step 4: Bridging ${receivedAmount} tokens back to EVM...`);

            const bridgeBackResult = await coreWriter.bridgeToEvm({
                token: decision.tokenToBuy,
                amount: receivedAmount,
                to: await this.signer.getAddress()
            });

            if (!bridgeBackResult.success) {
                return {
                    success: false,
                    error: `Bridge back to EVM failed: ${bridgeBackResult.error}`,
                    hash: bridgeBackResult.hash,
                    strategyUsed: 'statistical-arb'
                };
            }

            // Step 5: If we received HYPE and original target was WHYPE, wrap the exact received amount to WHYPE
            let finalHash = bridgeBackResult.hash;
            if (decision.tokenToBuy === '0x2222222222222222222222222222222222222222' &&
                target.base_token === 'wHYPE') {
                console.log(`Step 5: Wrapping exactly ${receivedAmount} received HYPE to WHYPE for trading...`);

                // Convert core wei back to EVM format for wrapping
                const evmAmount = await this.precompileUtils.weiToEvm(decision.tokenToBuy, receivedAmount);
                const wrapResult = await this.wrapWhype(evmAmount);

                if (wrapResult.success && wrapResult.hash) {
                    console.log(`   ${receivedAmount} HYPE wrapped to WHYPE: ${wrapResult.hash}`);
                    finalHash = wrapResult.hash; // Use wrap transaction as final hash
                } else {
                    console.warn(`   Warning: Failed to wrap HYPE to WHYPE: ${wrapResult.error}`);
                    // Don't fail the entire operation for wrapping issues
                }
            }

            console.log(`✅ Statistical arbitrage completed successfully!`);
            console.log(`   Final transaction: ${finalHash}`);

            return {
                success: true,
                hash: finalHash, // Return the final transaction (bridge or wrap)
                strategyUsed: 'statistical-arb'
            };

        } catch (error) {
            return {
                success: false,
                error: `Statistical arb execution failed: ${(error as Error).message}`,
                strategyUsed: 'statistical-arb'
            };
        }
    }

    /**
     * Execute simple rebalancing using dual bridging to mirror both layers at 50-50
     * No swaps needed - just bridge assets in both directions simultaneously
     */
    private async executeSimpleRebalancing(target: TargetConfig, decision: RebalanceDecision): Promise<{
        success: boolean;
        hash?: string;
        error?: string;
        strategyUsed?: string;
    }> {
        try {
            if (!decision.additionalData) {
                return {
                    success: false,
                    error: 'Missing bridge amounts data for simple rebalancing',
                    strategyUsed: 'simple-dual-bridge'
                };
            }

            const {
                baseBridgeToCore = 0n,
                baseBridgeToEvm = 0n,
                quoteBridgeToCore = 0n,
                quoteBridgeToEvm = 0n,
                baseToken,
                quoteToken
            } = decision.additionalData;

            if (!baseToken || !quoteToken) {
                return {
                    success: false,
                    error: 'Missing token addresses in bridge data',
                    strategyUsed: 'simple-dual-bridge'
                };
            }

            const coreWriterUtils = await import('./CoreWriterUtils.js').then(m => m.CoreWriterUtils);
            const coreWriter = new coreWriterUtils(this.provider, this.signer);

            console.log('Simple rebalancing: multi-directional bridge strategy');
            console.log(`Base token: ${ethers.formatEther(baseBridgeToCore)} EVM→Core, ${ethers.formatEther(baseBridgeToEvm)} Core→EVM`);
            console.log(`Quote token: ${ethers.formatEther(quoteBridgeToCore)} EVM→Core, ${ethers.formatEther(quoteBridgeToEvm)} Core→EVM`);

            const results: Array<{ success: boolean; hash?: string; error?: string }> = [];

            // Execute base token bridge from EVM → Core (if needed)
            if (baseBridgeToCore > 0n) {
                console.log(`Bridging ${ethers.formatEther(baseBridgeToCore)} base tokens EVM→Core...`);
                const bridgeResult = await coreWriter.bridgeToCore({
                    token: baseToken,
                    amount: baseBridgeToCore
                });
                results.push(bridgeResult);
                if (!bridgeResult.success) {
                    return { success: false, error: `Base EVM→Core bridge failed: ${bridgeResult.error}`, hash: bridgeResult.hash, strategyUsed: 'simple-dual-bridge' };
                }
            }

            // Execute base token bridge from Core → EVM (if needed)
            if (baseBridgeToEvm > 0n) {
                console.log(`Bridging ${ethers.formatEther(baseBridgeToEvm)} base tokens Core→EVM...`);
                const bridgeResult = await coreWriter.bridgeToEvm({
                    token: baseToken,
                    amount: baseBridgeToEvm,
                    to: await this.signer.getAddress()
                });
                results.push(bridgeResult);
                if (!bridgeResult.success) {
                    return { success: false, error: `Base Core→EVM bridge failed: ${bridgeResult.error}`, hash: bridgeResult.hash, strategyUsed: 'simple-dual-bridge' };
                }
            }

            // Execute quote token bridge from EVM → Core (if needed)
            if (quoteBridgeToCore > 0n) {
                console.log(`Bridging ${ethers.formatEther(quoteBridgeToCore)} quote tokens EVM→Core...`);
                const bridgeResult = await coreWriter.bridgeToCore({
                    token: quoteToken,
                    amount: quoteBridgeToCore
                });
                results.push(bridgeResult);
                if (!bridgeResult.success) {
                    return { success: false, error: `Quote EVM→Core bridge failed: ${bridgeResult.error}`, hash: bridgeResult.hash, strategyUsed: 'simple-dual-bridge' };
                }
            }

            // Execute quote token bridge from Core → EVM (if needed)
            if (quoteBridgeToEvm > 0n) {
                console.log(`Bridging ${ethers.formatEther(quoteBridgeToEvm)} quote tokens Core→EVM...`);
                const bridgeResult = await coreWriter.bridgeToEvm({
                    token: quoteToken,
                    amount: quoteBridgeToEvm,
                    to: await this.signer.getAddress()
                });
                results.push(bridgeResult);

                if (!bridgeResult.success) {
                    return { success: false, error: `Quote Core→EVM bridge failed: ${bridgeResult.error}`, hash: bridgeResult.hash, strategyUsed: 'simple-dual-bridge' };
                }
            }

            // Return success with the last transaction hash (or first if only one bridge)
            const lastSuccessfulResult = results.find(r => r.success && r.hash);

            return {
                success: true,
                hash: lastSuccessfulResult?.hash,
                strategyUsed: 'simple-dual-bridge'
            };

        } catch (error) {
            return {
                success: false,
                error: `Simple dual-bridge rebalancing failed: ${(error as Error).message}`,
                strategyUsed: 'simple-dual-bridge'
            };
        }
    }


    /**
     * Complete rebalancing workflow for a target configuration
     * Includes gas price checking, WHYPE unwrapping if needed, and execution
     */
    async performCompleteRebalancing(target: TargetConfig, gasConfig: {
        maxGasPriceGwei: number;
        nativeHypeReserve: number;
    }): Promise<{
        success: boolean;
        hash?: string;
        error?: string;
        decision?: RebalanceDecision;
        gasPrice?: number;
        whypeUnwrapped?: boolean;
    }> {
        try {
            // Step 1: Check gas price limits
            const gasCheck = await this.isGasPriceAcceptable(gasConfig.maxGasPriceGwei);
            if (!gasCheck.acceptable) {
                return {
                    success: false,
                    error: `Gas price too high: ${gasCheck.currentGwei} gwei > ${gasConfig.maxGasPriceGwei} gwei limit`,
                    gasPrice: gasCheck.currentGwei
                };
            }

            // Step 2: Analyze if rebalancing is needed
            const decision = await this.analyzeRebalanceNeed(target);
            if (!decision.needsRebalance) {
                return {
                    success: true,
                    error: decision.reason,
                    decision,
                    gasPrice: gasCheck.currentGwei
                };
            }

            // Step 3: Check if we need to unwrap WHYPE for gas (if dealing with WHYPE in statistical arb)
            let whypeUnwrapped = false;
            if (target.statistical_arb && target.base_token === 'wHYPE') {
                const requiredGas = BigInt(Math.floor(gasConfig.nativeHypeReserve * 1e18)); // Reserve amount in wei
                const whypeCheck = await this.shouldUnwrapWhype(requiredGas);

                if (whypeCheck.shouldUnwrap) {
                    const unwrapAmount = requiredGas - whypeCheck.hypeBalance;
                    const unwrapResult = await this.unwrapWhype(unwrapAmount);

                    if (unwrapResult.success) {
                        whypeUnwrapped = true;
                    } else {
                        return {
                            success: false,
                            error: `Failed to unwrap WHYPE for gas: ${unwrapResult.error}`,
                            decision,
                            gasPrice: gasCheck.currentGwei
                        };
                    }
                }
            }

            // Step 4: Execute the rebalancing
            const executionResult = await this.executeRebalancing(target, decision);

            return {
                success: executionResult.success,
                hash: executionResult.hash,
                error: executionResult.error,
                decision,
                gasPrice: gasCheck.currentGwei,
                whypeUnwrapped
            };

        } catch (error) {
            return {
                success: false,
                error: `Complete rebalancing failed: ${(error as Error).message}`
            };
        }
    }

    /**
     * Get current gas price in gwei
     */
    async getCurrentGasPriceGwei(): Promise<number> {
        try {
            const feeData = await this.provider.getFeeData();
            const gasPrice = feeData.gasPrice || 0n;
            return Number(gasPrice) / 1e9; // Convert wei to gwei
        } catch (error) {
            console.warn(`Failed to get gas price: ${error}`);
            return 0;
        }
    }

    /**
     * Check if gas price is within acceptable limits
     */
    async isGasPriceAcceptable(maxGasPriceGwei: number): Promise<{ acceptable: boolean; currentGwei: number }> {
        const currentGwei = await this.getCurrentGasPriceGwei();

        return {
            acceptable: currentGwei <= maxGasPriceGwei,
            currentGwei
        };
    }
}
