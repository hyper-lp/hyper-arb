/**
 * CoreWriter Utilities for HyperEVM
 * Handles bridging operations between EVM and HyperCore
 */

import { ethers } from 'ethers';
import { PrecompileUtils } from './PrecompileUtils.js';

// CoreWriter contract address (from hyper-evm-lib CoreWriterLib.sol)
const CORE_WRITER_ADDRESS = '0x3333333333333333333333333333333333333333';

// HyperCore system addresses (from HLConstants.sol) 
const HYPE_SYSTEM_ADDRESS = '0x2222222222222222222222222222222222222222';
const BASE_SYSTEM_ADDRESS = 0x2000000000000000000000000000000000000000n;
const HYPE_EVM_EXTRA_DECIMALS = 10;

// CoreWriter ABI - basic functions we need
const CORE_WRITER_ABI = [
    "function sendRawAction(bytes calldata rawAction) external"
];

// HyperCore action types (from HLConstants.sol)
const CORE_ACTIONS = {
    LIMIT_ORDER: 1,
    SPOT_SEND: 6,
} as const;

// Order constants from HLConstants.sol
const LIMIT_ORDER_TIF_IOC = 3; // Immediate or Cancel
const LIMIT_ORDER_TIF_GTC = 2; // Good Till Cancel

export interface TransactionResult {
    hash: string;
    success: boolean;
    gasUsed?: bigint;
    error?: string;
}

export interface BridgeParams {
    token: string;
    amount: bigint;
    to?: string; // For bridgeToEvm, recipient address (used by bridge mechanism, not direct transfer)
}

export interface SwapParams {
    token: string;
    amount: bigint;
}

export interface SwapResult {
    success: boolean;
    hash?: string;
    error?: string;
    expectedAmount?: bigint; // Expected output amount for status checking
}

export interface SwapStatus {
    completed: boolean;
    actualAmount?: bigint;
    pendingAmount?: bigint;
}

export class CoreWriterUtils {
    private readProvider: ethers.Provider;
    private signer: ethers.Signer;
    private coreWriter: ethers.Contract;
    private precompileUtils: PrecompileUtils;

    constructor(readProvider: ethers.Provider, signer: ethers.Signer) {
        this.readProvider = readProvider;
        this.signer = signer;
        this.coreWriter = new ethers.Contract(CORE_WRITER_ADDRESS, CORE_WRITER_ABI, signer);
        this.precompileUtils = new PrecompileUtils(readProvider);
    }

    /**
     * Bridge tokens from EVM to HyperCore
     * Transfers tokens to the appropriate system address
     */
    async bridgeToCore(params: BridgeParams): Promise<TransactionResult> {
        try {
            const { token, amount } = params;
            const tokenIndex = await this.precompileUtils.getTokenIndex(token);
            const isHypeToken = await this.isHype(Number(tokenIndex));

            if (isHypeToken) {
                // For HYPE, transfer native ETH to HYPE system address
                // Amount conversion handled by HLConstants.HYPE_EVM_EXTRA_DECIMALS
                const coreAmount = amount / (10n ** BigInt(HYPE_EVM_EXTRA_DECIMALS));
                return await this.transferToSystemAddress(HYPE_SYSTEM_ADDRESS, coreAmount, true);
            } else {
                // For ERC20 tokens, transfer to token-specific system address
                const systemAddress = await this.getSystemAddress(Number(tokenIndex));
                return await this.transferToSystemAddress(systemAddress, amount, false, token);
            }
        } catch (error) {
            return {
                hash: '',
                success: false,
                error: (error as Error).message
            };
        }
    }

    /**
     * Bridge tokens from HyperCore to EVM
     * Uses spotSend to system address to trigger bridge mechanism
     * Based on CoreWriterLib.sol bridgeToEvm implementation
     */
    async bridgeToEvm(params: BridgeParams): Promise<TransactionResult> {
        try {
            const { token, amount, to } = params;

            if (!to) {
                throw new Error('Recipient address required for bridgeToEvm');
            }

            const tokenIndex = await this.precompileUtils.getTokenIndex(token);

            // Convert EVM amount to HyperCore wei format
            const weiAmount = await this.evmToWei(token, amount);

            // Get system address for this token (this triggers the bridge)
            const systemAddress = await this.getSystemAddress(Number(tokenIndex));

            // Execute spotSend to SYSTEM ADDRESS to trigger bridging to EVM
            // The bridge mechanism will mint/transfer tokens to the recipient on EVM side
            return await this.spotSend(systemAddress, Number(tokenIndex), weiAmount);

        } catch (error) {
            return {
                hash: '',
                success: false,
                error: (error as Error).message
            };
        }
    }

    /**
     * Execute spotSend action on HyperCore
     * Transfers tokens between accounts on HyperCore
     */
    async spotSend(to: string, tokenIndex: number, amountWei: bigint): Promise<TransactionResult> {
        try {
            // Prevent self-transfers
            const signerAddress = await this.signer.getAddress();
            if (to.toLowerCase() === signerAddress.toLowerCase()) {
                throw new Error('Cannot send to self');
            }

            // Encode spotSend action: action_type(1) + SPOT_SEND_ACTION(6) + encode(to, token, amount)
            const actionData = ethers.AbiCoder.defaultAbiCoder().encode(
                ['address', 'uint64', 'uint64'],
                [to, tokenIndex, amountWei]
            );

            const rawAction = ethers.solidityPacked(
                ['uint8', 'uint24', 'bytes'],
                [1, CORE_ACTIONS.SPOT_SEND, actionData]
            );

            // Send transaction
            const tx = await this.coreWriter.sendRawAction(rawAction);
            const receipt = await tx.wait();

            return {
                hash: tx.hash,
                success: receipt.status === 1,
                gasUsed: receipt.gasUsed
            };

        } catch (error) {
            return {
                hash: '',
                success: false,
                error: (error as Error).message
            };
        }
    }

    /**
     * Transfer tokens to system address (unified bridge logic)
     */
    private async transferToSystemAddress(
        systemAddress: string,
        amount: bigint,
        isNative: boolean,
        tokenAddress?: string
    ): Promise<TransactionResult> {
        try {
            let tx: ethers.TransactionResponse;

            if (isNative) {
                // Native token transfer (HYPE)
                tx = await this.signer.sendTransaction({
                    to: systemAddress,
                    value: amount
                });
            } else if (tokenAddress) {
                // ERC20 token transfer
                const erc20Abi = [
                    "function transfer(address to, uint256 amount) external returns (bool)"
                ];
                const tokenContract = new ethers.Contract(tokenAddress, erc20Abi, this.signer);
                tx = await tokenContract.transfer(systemAddress, amount);
            } else {
                throw new Error('Token address required for ERC20 transfers');
            }

            const receipt = await tx.wait();
            return {
                hash: tx.hash,
                success: receipt?.status === 1,
                gasUsed: receipt?.gasUsed
            };
        } catch (error) {
            return {
                hash: '',
                success: false,
                error: (error as Error).message
            };
        }
    }

    /**
     * Convert EVM amount to HyperCore wei format
     * Uses HLConversions.evmToWei() logic from hyper-evm-lib
     */
    private async evmToWei(tokenAddress: string, evmAmount: bigint): Promise<bigint> {
        return await this.precompileUtils.evmToWei(tokenAddress, evmAmount);
    }


    /**
     * Get system address for a token index
     * Uses HyperLiquid's system address calculation from HLConstants
     */
    private async getSystemAddress(tokenIndex: number): Promise<string> {
        if (await this.isHype(tokenIndex)) {
            return HYPE_SYSTEM_ADDRESS;
        }

        // Calculate system address: BASE_SYSTEM_ADDRESS + tokenIndex
        const systemAddress = BASE_SYSTEM_ADDRESS + BigInt(tokenIndex);
        return `0x${systemAddress.toString(16).padStart(40, '0')}`;
    }

    /**
     * Check if token is HYPE based on HLConstants.isHype() logic
     */
    private async isHype(tokenIndex: number): Promise<boolean> {
        const network = await this.readProvider.getNetwork();
        const chainId = network.chainId;

        // From HLConstants.hypeTokenIndex()
        const hypeIndex = chainId === 998n ? 1105 : 150;
        return tokenIndex === hypeIndex;
    }

    /**
     * Estimate gas for a CoreWriter operation
     */
    async estimateGas(action: 'bridgeToCore' | 'bridgeToEvm', params: BridgeParams): Promise<bigint> {
        // This would require actual transaction simulation
        // For now, return reasonable estimates based on operation type
        if (action === 'bridgeToCore') {
            const tokenIndex = await this.precompileUtils.getTokenIndex(params.token);
            const isHypeToken = await this.isHype(Number(tokenIndex));
            return isHypeToken ? 21000n : 50000n; // ETH transfer vs ERC20 transfer
        } else {
            return 100000n; // CoreWriter action
        }
    }

    /**
     * Check if wallet has sufficient HYPE balance for non-HYPE token operations
     */
    async checkHypeBalance(requiredGas: bigint): Promise<boolean> {
        try {
            const signerAddress = await this.signer.getAddress();
            const balance = await this.readProvider.getBalance(signerAddress);
            return balance >= requiredGas;
        } catch {
            return false;
        }
    }

    /*//////////////////////////////////////////////////////////////
                            SWAP FUNCTIONS
    //////////////////////////////////////////////////////////////*/

    /**
     * Swap asset to USDC using market sell order
     * @param params - Token address and amount to sell
     * @returns Transaction result with expected USDC output
     */
    async swapAssetToUsdc(params: SwapParams): Promise<SwapResult> {
        try {
            const tokenIndex = await this.precompileUtils.getTokenIndex(params.token);

            // Convert EVM amount to HyperCore wei
            const coreWei = await this.evmToWei(params.token, params.amount);
            if (coreWei === 0n) {
                return { success: false, error: 'Amount too small after conversion' };
            }

            // Convert to sz units for the order
            const szNative = await this.weiToSz(params.token, coreWei);
            if (szNative === 0n) {
                return { success: false, error: 'Trade size too small' };
            }

            // Get token info for decimal precision
            const tokenInfo = await this.precompileUtils.getTokenInfo(params.token);

            // Apply Python truncation logic: truncate(float(balance["total"]), sz_decimals)
            const szTruncated = this.truncate(szNative, tokenInfo.szDecimals);

            // Scale to 8 decimals (order encoding expects 8-decimal format despite MyHyperVault.sol)
            const scaleExponent = 8 - tokenInfo.szDecimals;
            const sz = szTruncated * (10n ** BigInt(scaleExponent));

            // Get asset ID for this token/USDC trading pair
            const assetId = await this.getAssetId(params.token);

            // Use raw spot price format directly (following MyHyperVault.sol pattern)
            // Raw spot price already follows HyperLiquid format: max (8-szDecimals) decimal places  
            const currentPrice = await this.getSpotPrice(params.token);
            const sellPrice = this.formatPrice((currentPrice * 99n) / 100n, tokenInfo.szDecimals, tokenInfo.weiDecimals, params.token); // -1% for aggressive sell

            // Use sz units in native precision (following HyperLiquid tick/lot size rules)
            const actionData = this.encodeLimitOrder({
                assetId,
                isBuy: false,
                limitPx: sellPrice,
                sz,
                reduceOnly: false,
                tif: LIMIT_ORDER_TIF_IOC,
                cloid: 0n // Try with cloid=0 instead of block number
            });

            const tx = await this.coreWriter.sendRawAction(actionData);
            const receipt = await tx.wait();

            // Estimate expected USDC output (approximate)
            const spotPrice = await this.getSpotPrice(params.token);
            // USDC is token index 0 and typically has 8 decimals in HyperCore
            const expectedUsdc = (coreWei * spotPrice) / (10n ** 8n); // USDC core wei format

            return {
                success: true,
                hash: receipt.transactionHash,
                expectedAmount: expectedUsdc
            };

        } catch (error) {
            return {
                success: false,
                error: `Swap failed: ${(error as Error).message}`
            };
        }
    }

    /**
     * Swap USDC to asset using market buy order
     * @param params - Target token address and USDC amount to spend
     * @returns Transaction result
     */
    async swapUsdcToAsset(params: SwapParams): Promise<SwapResult> {
        try {
            const tokenIndex = await this.precompileUtils.getTokenIndex(params.token);

            // Get token info and spot price - following MyHyperVault.sol pattern
            const tokenInfo = await this.precompileUtils.getTokenInfo(params.token);
            const spotIndex = await this.precompileUtils.getSpotIndex(params.token);
            const rawSpotPx = await this.precompileUtils.getSpotPx(spotIndex);

            if (rawSpotPx === 0n) {
                return { success: false, error: 'Spot price unavailable' };
            }

            // Convert USDC amount from EVM (6d) to spot format (8d)
            const usdcSpot = params.amount * (10n ** 2n);

            // Calculate sz directly: sz = usdcSpot / spotPx (MyHyperVault.sol line 91)
            const szNative = usdcSpot / rawSpotPx;

            // Apply Python truncation logic: truncate(float(balance["total"]), sz_decimals)
            const szTruncated = this.truncate(szNative, tokenInfo.szDecimals);

            // Scale to 8 decimals (order encoding expects 8-decimal format)
            const scaleExponent = 8 - tokenInfo.szDecimals;
            const sz = szTruncated * (10n ** BigInt(scaleExponent));

            if (sz === 0n) {
                return { success: false, error: 'Target trade size too small' };
            }

            // Get asset ID for this token/USDC trading pair
            const assetId = await this.getAssetId(params.token);

            const currentPrice = await this.getSpotPrice(params.token);
            // Use raw spot price format directly (following MyHyperVault.sol pattern)  
            // Raw spot price already follows HyperLiquid format: max (8-szDecimals) decimal places
            const limitPx = this.formatPrice((currentPrice * 101n) / 100n, tokenInfo.szDecimals, tokenInfo.weiDecimals, params.token); // +1% for aggressive buy
            // Place IOC buy order (sz field expects sz units in native precision)
            const actionData = this.encodeLimitOrder({
                assetId,
                isBuy: true,
                limitPx,
                sz,
                reduceOnly: false,
                tif: LIMIT_ORDER_TIF_IOC,
                cloid: 0n // Try with cloid=0 instead of block number
            });

            const tx = await this.coreWriter.sendRawAction(actionData);
            const receipt = await tx.wait();

            // Calculate expected wei amount: native sz units → wei units
            const scale = 10n ** BigInt(tokenInfo.weiDecimals - tokenInfo.szDecimals);
            const expectedCoreWei = szNative * scale;

            return {
                success: true,
                hash: receipt.transactionHash,
                expectedAmount: expectedCoreWei
            };

        } catch (error) {
            return {
                success: false,
                error: `Swap failed: ${(error as Error).message}`
            };
        }
    }

    /**
     * Check if swap order has been completely filled
     * @param tokenAddress - Token to check balance for
     * @param expectedAmount - Expected amount from the swap
     * @returns Swap completion status
     */
    async checkSwapCompleted(tokenAddress: string, expectedAmount: bigint): Promise<SwapStatus> {
        try {
            const balance = await this.precompileUtils.getSpotBalance(
                await this.signer.getAddress(),
                tokenAddress
            );

            const actualAmount = balance.total;
            const tolerance = expectedAmount / 100n; // 1% tolerance

            const completed = actualAmount >= (expectedAmount - tolerance);

            return {
                completed,
                actualAmount,
                pendingAmount: completed ? 0n : (expectedAmount - actualAmount)
            };

        } catch (error) {
            return {
                completed: false,
                actualAmount: 0n,
                pendingAmount: expectedAmount
            };
        }
    }

    /*//////////////////////////////////////////////////////////////
                        HELPER FUNCTIONS FOR SWAPS
    //////////////////////////////////////////////////////////////*/

    /**
     * Convert core wei to trade size units (sz)
     * Uses HLConversions.weiToSz() logic
     */
    private async weiToSz(tokenAddress: string, coreWei: bigint): Promise<bigint> {
        return await this.precompileUtils.weiToSz(tokenAddress, coreWei);
    }

    /**
     * Get asset ID for trading (implements HLConversions.spotToAssetId)
     * AssetID = SpotIndex + 10000 (from HLConversions.spotToAssetId)
     */
    private async getAssetId(tokenAddress: string): Promise<number> {
        const spotIndex = await this.precompileUtils.getSpotIndex(tokenAddress);
        return Number(spotIndex) + 10000; // HLConversions.spotToAssetId logic
    }

    /**
     * Get current spot price for a token
     */
    private async getSpotPrice(tokenAddress: string): Promise<bigint> {
        return await this.precompileUtils.getSpotPrice(tokenAddress);
    }

    /**
     * Format price using Python logic with sz constraint.
     * Steps:
     *  - Python rule: float(f"{px:.6g}") for BTC, float(f"{px:.5g}") for others
     *  - Enforce HyperLiquid precision: max (8 - szDecimals) decimals
     * Input: price in 8-decimal integer format
     * Output: price in 8-decimal integer format
     */
    private formatPrice(price: bigint, szDecimals: number, weiDecimals: number, tokenAddress: string): bigint {
        const p = Number(price) / 1e8;

        const isBtc = tokenAddress.toLowerCase() === '0x9fdbda0a5e284c32744d2f17ee5c74b284993463';
        const isEth = tokenAddress.toLowerCase() === '0xbe6727b535545c67d5caa73dea54865b92cf7907';
        const isUsdt = tokenAddress.toLowerCase() === '0xb8ce59fc3717ada4c02eadf9682a9e934f625ebb';

        let px_out: bigint;

        if (isBtc) {
            // --- Significant digits rounding ---
            // Count digits before decimal
            const absP = Math.abs(p);
            const digitsBeforeDecimal = absP < 1 ? 0 : Math.floor(Math.log10(absP)) + 1;
            // Number of decimals to keep for 6 significant digits
            const decimalsToKeep = Math.max(0, 6 - digitsBeforeDecimal);

            // Round to decimalsToKeep
            const factor = Math.pow(10, decimalsToKeep);
            const pFinal = Math.round(p * factor) / factor;

            px_out = BigInt(Math.round(pFinal * 1e8));
        } else if (isEth || isUsdt) {
            // --- Significant digits rounding ---
            // Count digits before decimal
            const absP = Math.abs(p);
            const digitsBeforeDecimal = absP < 1 ? 0 : Math.floor(Math.log10(absP)) + 1;
            // Number of decimals to keep for 6 significant digits
            const decimalsToKeep = Math.max(0, 5 - digitsBeforeDecimal);

            // Round to decimalsToKeep
            const factor = Math.pow(10, decimalsToKeep);
            const pFinal = Math.round(p * factor) / factor;
            px_out = BigInt(Math.round(pFinal * 1e8));

        } else {
            // HL spot: round to 8 - szDecimals decimals (as you had)
            const maxDecimals = Math.max(0, 8 - szDecimals);
            const factor = Math.pow(10, maxDecimals);
            const pFinal = Math.round(p * factor) / factor;
            px_out = BigInt(Math.round(pFinal * 1e8));
        }

        return px_out;
    }

    /**
     * Truncate number to specified decimal places using floor
     * Python equivalent: math.floor(number * factor) / factor where factor = 10.0 ** decimals
     * Input number is assumed to be in sz scale with szDecimals implicit decimal places
     */
    private truncate(number: bigint, decimals: number): bigint {
        if (decimals <= 0) return number;

        // Convert to float in appropriate scale (sz units have szDecimals precision)
        const scale = 10n ** BigInt(decimals);
        const numberFloat = Number(number) / Number(scale);

        // Apply Python truncation logic: floor(number * factor) / factor
        const factor = Math.pow(10, decimals);
        const truncated = Math.floor(numberFloat * factor) / factor;

        // Convert back to BigInt in original sz scale
        return BigInt(Math.round(truncated * Number(scale)));
    }

    /**
     * Encode limit order action for CoreWriter
     */
    private encodeLimitOrder(params: {
        assetId: number;
        isBuy: boolean;
        limitPx: bigint;
        sz: bigint;
        reduceOnly: boolean;
        tif: number;
        cloid: bigint;
    }): string {
        // Encode as: uint8(1) + LIMIT_ORDER_ACTION + abi.encode(order_data)
        const abiCoder = ethers.AbiCoder.defaultAbiCoder();

        const orderData = abiCoder.encode(
            ['uint32', 'bool', 'uint64', 'uint64', 'bool', 'uint8', 'uint128'],
            [
                params.assetId,
                params.isBuy,
                params.limitPx,
                params.sz,
                params.reduceOnly,
                params.tif,
                params.cloid
            ]
        );

        // Create the full action: actionType(1) + LIMIT_ORDER(1) + encoded_data
        return ethers.solidityPacked(
            ['uint8', 'uint24', 'bytes'],
            [1, CORE_ACTIONS.LIMIT_ORDER, orderData]
        );
    }
}
