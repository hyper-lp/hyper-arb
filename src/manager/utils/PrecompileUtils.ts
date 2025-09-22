/**
 * PrecompileUtils for HyperEVM
 * TypeScript implementation based on hyper-evm-lib PrecompileLib.sol
 * Provides direct precompile access without assumptions or fallbacks
 */

import { ethers } from 'ethers';
import { SpotBalance, TokenInfo, SpotInfo } from '../types/interfaces.js';

// Precompile addresses from HLConstants.sol
const PRECOMPILE_ADDRESSES = {
    POSITION: '0x0000000000000000000000000000000000000800',
    SPOT_BALANCE: '0x0000000000000000000000000000000000000801',
    VAULT_EQUITY: '0x0000000000000000000000000000000000000802',
    WITHDRAWABLE: '0x0000000000000000000000000000000000000803',
    DELEGATIONS: '0x0000000000000000000000000000000000000804',
    DELEGATOR_SUMMARY: '0x0000000000000000000000000000000000000805',
    MARK_PX: '0x0000000000000000000000000000000000000806',
    ORACLE_PX: '0x0000000000000000000000000000000000000807',
    SPOT_PX: '0x0000000000000000000000000000000000000808',
    L1_BLOCK_NUMBER: '0x0000000000000000000000000000000000000809',
    PERP_ASSET_INFO: '0x000000000000000000000000000000000000080a',
    SPOT_INFO: '0x000000000000000000000000000000000000080b',
    TOKEN_INFO: '0x000000000000000000000000000000000000080C',
    TOKEN_SUPPLY: '0x000000000000000000000000000000000000080D',
    BBO: '0x000000000000000000000000000000000000080e',
    ACCOUNT_MARGIN_SUMMARY: '0x000000000000000000000000000000000000080F',
    CORE_USER_EXISTS: '0x0000000000000000000000000000000000000810'
} as const;

// TokenRegistry address from PrecompileLib.sol
const TOKEN_REGISTRY_ADDRESS = '0x0b51d1A9098cf8a72C325003F44C194D41d7A85B';

const TOKEN_REGISTRY_ABI = [
    "function getTokenIndex(address evmContract) external view returns (uint32 index)"
];

export class PrecompileUtils {
    private provider: ethers.Provider;
    private tokenRegistry: ethers.Contract;

    constructor(provider: ethers.Provider) {
        this.provider = provider;
        this.tokenRegistry = new ethers.Contract(TOKEN_REGISTRY_ADDRESS, TOKEN_REGISTRY_ABI, provider);
    }

    /**
     * Get HYPE token index based on chain ID
     * Based on HLConstants.hypeTokenIndex()
     */
    async getHypeTokenIndex(): Promise<bigint> {
        const network = await this.provider.getNetwork();
        const chainId = network.chainId;

        // From HLConstants.hypeTokenIndex()
        const hypeIndex = chainId === 998n ? 1105 : 150;
        return BigInt(hypeIndex);
    }

    /**
     * Check if an address is HYPE (native or wrapped)
     */
    isHypeAddress(tokenAddress: string): boolean {
        const addr = tokenAddress.toLowerCase();
        return addr === '0x2222222222222222222222222222222222222222' || // Native HYPE
            addr === '0x5555555555555555555555555555555555555555';   // WHYPE
    }

    /**
     * Get the canonical HYPE address for price fetching
     * Both native HYPE and WHYPE should use the same price
     */
    getCanonicalHypeAddress(tokenAddress: string): string {
        if (this.isHypeAddress(tokenAddress)) {
            return '0x2222222222222222222222222222222222222222'; // Always use native HYPE for price
        }
        return tokenAddress;
    }

    /**
     * Get token index from TokenRegistry or HYPE constants
     * Based on PrecompileLib.getTokenIndex() with HYPE handling
     */
    async getTokenIndex(tokenAddress: string): Promise<bigint> {
        // Handle HYPE tokens using HLConstants.hypeTokenIndex()
        if (this.isHypeAddress(tokenAddress)) {
            return await this.getHypeTokenIndex();
        }

        // For all other tokens, use TokenRegistry
        const index = await this.tokenRegistry.getTokenIndex(tokenAddress);
        return BigInt(index);
    }

    /**
     * Get spot balance for a user and token
     * Based on PrecompileLib.spotBalance() with HYPE handling
     */
    async getSpotBalance(userAddress: string, tokenAddress: string): Promise<SpotBalance> {
        // Use canonical HYPE address for balance lookups
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenIndex = await this.getTokenIndex(canonicalAddress);
        return await this.getSpotBalanceByIndex(userAddress, tokenIndex);
    }

    /**
     * Get spot balance by token index
     * Direct precompile call
     */
    async getSpotBalanceByIndex(userAddress: string, tokenIndex: bigint): Promise<SpotBalance> {
        const callData = ethers.AbiCoder.defaultAbiCoder().encode(
            ['address', 'uint64'],
            [userAddress, tokenIndex]
        );

        const result = await this.provider.call({
            to: PRECOMPILE_ADDRESSES.SPOT_BALANCE,
            data: callData
        });

        const decoded = ethers.AbiCoder.defaultAbiCoder().decode(
            ['uint64', 'uint64', 'uint64'],
            result
        );

        return {
            total: decoded[0],
            hold: decoded[1],
            entryNtl: decoded[2]
        };
    }

    /**
     * Get TokenInfo for a token
     * Based on PrecompileLib.tokenInfo() with HYPE handling
     */
    async getTokenInfo(tokenAddress: string): Promise<TokenInfo> {
        // Use canonical HYPE address for token info lookups
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenIndex = await this.getTokenIndex(canonicalAddress);
        return await this.getTokenInfoByIndex(tokenIndex);
    }

    /**
     * Get TokenInfo by token index
     * Direct precompile call - uses tuple format as discovered by debug analysis
     */
    async getTokenInfoByIndex(tokenIndex: bigint): Promise<TokenInfo> {
        const callData = ethers.AbiCoder.defaultAbiCoder().encode(['uint64'], [tokenIndex]);

        const result = await this.provider.call({
            to: PRECOMPILE_ADDRESSES.TOKEN_INFO,
            data: callData
        });

        // Use tuple format - this is what actually works with the precompile
        const decoded = ethers.AbiCoder.defaultAbiCoder().decode(
            ['tuple(string,uint64[],uint64,address,address,uint8,uint8,int8)'],
            result
        );

        const tokenInfo = decoded[0];
        return {
            name: tokenInfo[0],
            spots: tokenInfo[1],
            deployerTradingFeeShare: tokenInfo[2],
            deployer: tokenInfo[3],
            evmContract: tokenInfo[4],
            szDecimals: Number(tokenInfo[5]),
            weiDecimals: Number(tokenInfo[6]),
            evmExtraWeiDecimals: Number(tokenInfo[7])
        };
    }

    /**
     * Get SpotInfo for a spot index
     * Based on PrecompileLib.spotInfo() - uses tuple format like TokenInfo
     */
    async getSpotInfo(spotIndex: bigint): Promise<SpotInfo> {
        const callData = ethers.AbiCoder.defaultAbiCoder().encode(['uint64'], [spotIndex]);

        const result = await this.provider.call({
            to: PRECOMPILE_ADDRESSES.SPOT_INFO,
            data: callData
        });

        // Try tuple format first, fallback to individual fields if needed
        try {
            const decoded = ethers.AbiCoder.defaultAbiCoder().decode(
                ['tuple(string,uint64[2])'],
                result
            );
            const spotInfo = decoded[0];
            return {
                name: spotInfo[0],
                tokens: [spotInfo[1][0], spotInfo[1][1]]
            };
        } catch (error) {
            // Fallback to individual fields if tuple fails
            const decoded = ethers.AbiCoder.defaultAbiCoder().decode(
                ['string', 'uint64[2]'],
                result
            );
            return {
                name: decoded[0],
                tokens: [decoded[1][0], decoded[1][1]]
            };
        }
    }

    /**
     * Get spot price for a spot index
     * Based on PrecompileLib.spotPx()
     */
    async getSpotPx(spotIndex: bigint): Promise<bigint> {
        const callData = ethers.AbiCoder.defaultAbiCoder().encode(['uint64'], [spotIndex]);

        const result = await this.provider.call({
            to: PRECOMPILE_ADDRESSES.SPOT_PX,
            data: callData
        });

        const decoded = ethers.AbiCoder.defaultAbiCoder().decode(['uint64'], result);
        return decoded[0];
    }

    /**
     * Get spot index for a token/USDC market
     * Based on PrecompileLib.getSpotIndex() - finds market with USDC as quote
     */
    async getSpotIndex(tokenAddress: string): Promise<bigint> {
        // Use canonical HYPE address for spot index lookups
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenIndex = await this.getTokenIndex(canonicalAddress);
        const tokenInfo = await this.getTokenInfoByIndex(tokenIndex);

        // If only one spot market, return it
        if (tokenInfo.spots.length === 1) {
            return tokenInfo.spots[0];
        }

        // Find spot market with USDC (index 0) as quote token
        for (const spotIndex of tokenInfo.spots) {
            const spotInfo = await this.getSpotInfo(spotIndex);
            if (spotInfo.tokens[1] === 0n) { // USDC is token index 0
                return spotIndex;
            }
        }

        throw new Error(`No USDC spot market found for token ${canonicalAddress} (original: ${tokenAddress})`);
    }

    /**
     * Get spot price for a token in its USDC market
     * Based on PrecompileLib.spotPx(address) overload with HYPE/WHYPE handling
     */
    async getSpotPrice(tokenAddress: string): Promise<bigint> {
        // For both HYPE (0x2222...) and WHYPE (0x5555...), get HYPE price
        // WHYPE is 1:1 with HYPE, so they have the same price
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);

        const spotIndex = await this.getSpotIndex(canonicalAddress);
        const rawPrice = await this.getSpotPx(spotIndex);

        // Get normalized price using szDecimals (PrecompileLib.normalizedSpotPx logic)
        const tokenInfo = await this.getTokenInfo(canonicalAddress);
        const normalizedPrice = rawPrice * (10n ** BigInt(tokenInfo.szDecimals));

        return normalizedPrice;
    }

    /**
     * Convert Core wei to trade size (sz) units
     * Based on HLConversions.weiToSz() with HYPE handling
     */
    async weiToSz(tokenAddress: string, amountWei: bigint): Promise<bigint> {
        // Use canonical HYPE address for conversions
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenInfo = await this.getTokenInfo(canonicalAddress);
        const divisor = 10n ** BigInt(tokenInfo.weiDecimals - tokenInfo.szDecimals);
        return amountWei / divisor;
    }

    /**
     * Convert trade size (sz) to Core wei units
     * Based on HLConversions.szToWei() with HYPE handling
     */
    async szToWei(tokenAddress: string, sz: bigint): Promise<bigint> {
        // Use canonical HYPE address for conversions
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenInfo = await this.getTokenInfo(canonicalAddress);
        const multiplier = 10n ** BigInt(tokenInfo.weiDecimals - tokenInfo.szDecimals);
        return sz * multiplier;
    }

    /**
     * Convert EVM amount to Core wei
     * Based on HLConversions.evmToWei() with HYPE handling
     */
    async evmToWei(tokenAddress: string, evmAmount: bigint): Promise<bigint> {
        // Use canonical HYPE address for conversions
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenInfo = await this.getTokenInfo(canonicalAddress);

        if (tokenInfo.evmExtraWeiDecimals > 0) {
            return evmAmount / (10n ** BigInt(tokenInfo.evmExtraWeiDecimals));
        } else if (tokenInfo.evmExtraWeiDecimals < 0) {
            return evmAmount * (10n ** BigInt(-tokenInfo.evmExtraWeiDecimals));
        } else {
            return evmAmount;
        }
    }

    /**
     * Convert Core wei to EVM amount
     * Based on HLConversions.weiToEvm() with HYPE handling
     */
    async weiToEvm(tokenAddress: string, amountWei: bigint): Promise<bigint> {
        // Use canonical HYPE address for conversions
        const canonicalAddress = this.getCanonicalHypeAddress(tokenAddress);
        const tokenInfo = await this.getTokenInfo(canonicalAddress);

        if (tokenInfo.evmExtraWeiDecimals > 0) {
            return amountWei * (10n ** BigInt(tokenInfo.evmExtraWeiDecimals));
        } else if (tokenInfo.evmExtraWeiDecimals < 0) {
            return amountWei / (10n ** BigInt(-tokenInfo.evmExtraWeiDecimals));
        } else {
            return amountWei;
        }
    }

    /**
     * Check if user exists on HyperCore
     * Based on PrecompileLib.coreUserExists()
     */
    async coreUserExists(userAddress: string): Promise<boolean> {
        const callData = ethers.AbiCoder.defaultAbiCoder().encode(['address'], [userAddress]);

        const result = await this.provider.call({
            to: PRECOMPILE_ADDRESSES.CORE_USER_EXISTS,
            data: callData
        });

        const decoded = ethers.AbiCoder.defaultAbiCoder().decode(['bool'], result);
        return decoded[0];
    }
}