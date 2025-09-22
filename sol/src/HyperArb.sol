// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {ICoreWriter} from "./interfaces/ICore.sol";
import {IRouter} from "./interfaces/IRouter.sol";

/**
 * @title Arbitrage
 * @notice Arbitrage contract for HyperLP Protocol
 */
contract Arbitrage is Ownable, ReentrancyGuard {
    using SafeERC20 for IERC20;

    // ========================================
    // State Variables
    // ========================================

    address public constant CORE_WRITER =
        0x3333333333333333333333333333333333333333;

    address public router;

    // ========================================
    // Decimal Constants & Token Info
    // ========================================

    // Standard decimal configurations for HyperLiquid tokens
    struct TokenDecimals {
        uint8 szDecimals;
        uint8 weiDecimals;
        uint8 evmDecimals;
    }

    // Common token addresses and their decimal configurations
    address constant BTC_ADDRESS = 0x9FDBdA0A5e284c32744D2f17Ee5c74B284993463;
    address constant ETH_ADDRESS = 0xBe6727B535545C67d5cAa73dEa54865B92CF7907;
    address constant USDT_ADDRESS = 0xB8CE59FC3717ada4C02eaDF9682A9e934F625ebb;
    address constant HYPE_ADDRESS = 0x2222222222222222222222222222222222222222;
    address constant WHYPE_ADDRESS = 0x5555555555555555555555555555555555555555;

    // Token decimal mappings based on HyperLiquid standards
    mapping(address => TokenDecimals) private tokenDecimals;

    // ========================================
    // Decimal Functions
    // ========================================

    /**
     * @dev Initialize token decimal configurations
     */
    function initializeTokenDecimals() private {
        // BTC: szDecimals=5, weiDecimals=8, evmDecimals=18
        tokenDecimals[BTC_ADDRESS] = TokenDecimals(5, 8, 18);

        // ETH: szDecimals=5, weiDecimals=8, evmDecimals=18
        tokenDecimals[ETH_ADDRESS] = TokenDecimals(5, 8, 18);

        // USDT: szDecimals=2, weiDecimals=8, evmDecimals=6
        tokenDecimals[USDT_ADDRESS] = TokenDecimals(2, 8, 6);

        // HYPE: szDecimals=3, weiDecimals=8, evmDecimals=18
        tokenDecimals[HYPE_ADDRESS] = TokenDecimals(3, 8, 18);
        tokenDecimals[WHYPE_ADDRESS] = TokenDecimals(3, 8, 18);

        // Default for unknown tokens: szDecimals=3, weiDecimals=8, evmDecimals=18
    }

    /**
     * @dev Get token decimals configuration
     */
    function getTokenDecimals(address token)
        private
        view
        returns (TokenDecimals memory)
    {
        TokenDecimals memory decimals = tokenDecimals[token];
        // Return default if not configured
        if (decimals.szDecimals == 0) {
            return TokenDecimals(3, 8, 18); // Default configuration
        }
        return decimals;
    }

    /**
     * @dev Format price with HyperLiquid decimal constraints
     * Input: price in 8-decimal integer format
     * Output: price in 8-decimal integer format with proper rounding
     */
    function formatPrice(uint64 price, address token)
        private
        pure
        returns (uint64)
    {
        if (token == BTC_ADDRESS) {
            // BTC: 6 significant digits rounding
            return formatPriceWithSigDigits(price, 6);
        } else if (token == ETH_ADDRESS || token == USDT_ADDRESS) {
            // ETH/USDT: 5 significant digits rounding
            return formatPriceWithSigDigits(price, 5);
        } else {
            // Other tokens: use szDecimals constraint
            TokenDecimals memory decimals = getTokenDecimals(token);
            uint8 maxDecimals =
                decimals.szDecimals >= 8 ? 0 : 8 - decimals.szDecimals;
            return roundToDecimals(price, maxDecimals);
        }
    }

    /**
     * @dev Round price to specified number of significant digits
     */
    function formatPriceWithSigDigits(uint64 price, uint8 sigDigits)
        private
        pure
        returns (uint64)
    {
        if (price == 0) return 0;

        // Convert to floating point equivalent (price / 1e8)
        uint256 p = uint256(price);

        // Find number of digits before decimal
        uint8 digitsBeforeDecimal = 0;
        uint256 temp = p / 1e8;
        if (temp >= 1) {
            while (temp > 0) {
                digitsBeforeDecimal++;
                temp /= 10;
            }
        }

        // Calculate decimals to keep
        uint8 decimalsToKeep = sigDigits > digitsBeforeDecimal
            ? sigDigits - digitsBeforeDecimal
            : 0;

        return roundToDecimals(price, decimalsToKeep);
    }

    /**
     * @dev Round price to specified decimal places
     */
    function roundToDecimals(uint64 price, uint8 decimals)
        private
        pure
        returns (uint64)
    {
        if (decimals >= 8) return price;

        uint256 factor = 10 ** (8 - decimals);
        uint256 rounded = ((uint256(price) + factor / 2) / factor) * factor;

        return uint64(rounded);
    }

    /**
     * @dev Scale size from native precision to 8-decimal encoding format
     * @param size Size in native sz precision
     * @param token Token address for decimal lookup
     * @return Scaled size for order encoding (8 decimals)
     */
    function scaleSize(uint64 size, address token)
        private
        view
        returns (uint64)
    {
        TokenDecimals memory decimals = getTokenDecimals(token);

        if (decimals.szDecimals >= 8) {
            // If szDecimals >= 8, scale down
            uint8 scaleDown = decimals.szDecimals - 8;
            return size / uint64(10 ** scaleDown);
        } else {
            // Scale up to 8 decimals
            uint8 scaleUp = 8 - decimals.szDecimals;
            return size * uint64(10 ** scaleUp);
        }
    }

    /**
     * @dev Truncate size to token's native sz precision (floor operation)
     * @param size Size value to truncate
     * @param token Token address for decimal lookup
     * @return Truncated size in native precision
     */
    function truncateSize(uint64 size, address token)
        private
        view
        returns (uint64)
    {
        TokenDecimals memory decimals = getTokenDecimals(token);

        if (decimals.szDecimals == 0) return size;

        // Apply truncation (floor) at szDecimals precision
        uint256 factor = 10 ** decimals.szDecimals;
        uint256 sizeFloat = (uint256(size) * 1e18) / factor; // Scale to avoid precision loss
        uint256 truncated = (sizeFloat / 1e18) * 1e18; // Floor operation

        return uint64((truncated * factor) / 1e18);
    }

    event BridgeToCore(address token, uint256 amount);

    event BridgeToEvm(uint64 token, uint64 weiAmount);

    event CrossMarketTransfer(uint256 ntl, bool toPerp);

    event LimitOrder(
        uint32 assetId,
        bool isBuy,
        uint64 limitPx,
        uint64 size,
        bool reduceOnly,
        uint8 encodedTif,
        uint128 cloid
    );

    event APIWalletAdded(
        address indexed walletAddress, string indexed walletName
    );

    event SpotTransfer(
        address indexed to, uint64 indexed token, uint64 indexed weiAmount
    );

    event TokenSwap(
        address indexed tokenIn,
        address indexed tokenOut,
        uint256 amountIn,
        uint256 amountOut,
        address indexed to
    );

    event SystemAddressUpdated(
        address indexed oldAddress, address indexed newAddress
    );
    event RouterUpdated(address indexed oldRouter, address indexed newRouter);
    event KeeperUpdated(address indexed oldKeeper, address indexed newKeeper);
    event CancelLimitOrder(uint32 assetId, uint64 oid);
    // ========================================
    // Constructor
    // ========================================

    constructor() Ownable(msg.sender) {
        initializeTokenDecimals();
    }

    // ========================================
    // Admin Functions
    // ========================================

    function setRouter(address _router) external onlyOwner {
        require(_router != address(0), "Invalid router address");
        address oldRouter = router;
        router = _router;
        emit RouterUpdated(oldRouter, _router);
    }

    /**
     * @dev Add API wallet to HyperCore for Hyperliquid integration
     * @param walletAddress The API wallet address to add
     * @param walletName The name for the API wallet (empty string makes it the main API wallet/agent)
     * @notice This function can only be called once every 170 days for security
     */
    function addApiWallet(address walletAddress, string memory walletName)
        external
        onlyOwner
    {
        require(walletAddress != address(0), "Wallet address cannot be zero");

        // Construct the action data for adding API wallet (Action ID 9)
        bytes memory encodedAction = abi.encode(walletAddress, walletName);
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        // Action ID 9 (Add API wallet)
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x09;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit APIWalletAdded(walletAddress, walletName);
    }

    /**
     * @dev Bridge USDT from HyperCore to HyperEVM
     * @param tokenSystemAddress The token system address
     * @param token The token to bridge
     * @param weiAmount Amount of token to bridge (in wei)
     * @notice Uses spotSend action to transfer USDT from HyperCore to HyperEVM
     */
    function bridgeToEvm(
        address tokenSystemAddress,
        uint64 token,
        uint64 weiAmount
    ) external onlyOwner {
        require(weiAmount > 0, "Amount must be greater than 0");

        // Construct the action data for spot transfer (Action ID 5)
        bytes memory encodedAction =
            abi.encode(tokenSystemAddress, token, weiAmount);
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x06;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit BridgeToEvm(token, weiAmount);
    }

    /**
     * @dev Transfer USDT from HyperCore to HyperEVM
     * @param to The recipient address
     * @param token The token to bridge
     * @param weiAmount Amount of token to bridge (in wei)
     * @notice User action to transfer USDT from HyperCore to HyperEVM
     */
    function spotTransfer(address to, uint64 token, uint64 weiAmount)
        external
        onlyOwner
    {
        require(weiAmount > 0, "Amount must be greater than 0");

        bytes memory encodedAction = abi.encode(to, token, weiAmount);
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x06;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit SpotTransfer(to, token, weiAmount);
    }

    /**
     * @dev Bridge USDT from HyperEVM to HyperCore
     * @param tokenSystemAddress The token system address
     * @param token The token to bridge
     * @param amount Amount of token to bridge (in wei)
     * @notice Uses spotSend action to transfer USDT from HyperCore to HyperEVM
     */
    function bridgeToCore(
        address tokenSystemAddress,
        address token,
        uint256 amount
    ) external onlyOwner {
        require(amount > 0, "Amount must be greater than 0");

        IERC20(token).transfer(tokenSystemAddress, amount);

        emit BridgeToCore(token, amount);
    }

    /**
     * @dev Place a limit order on the spot market, mostly used for swaping asset to USDC
     * @param assetId The asset ID
     * @param isBuy True to buy, false to sell
     * @param limitPx The limit price (8-decimal format, will be formatted based on token)
     * @param size The size of the order (native sz precision, will be scaled to 8-decimal encoding)
     * @param reduceOnly True to reduce only, false to full order
     * @param encodedTif The time in force encoded as a uint8
     * @param cloid The cloid
     * @param tokenAddress The token address for decimal formatting (use address(0) for unknown)
     */
    function limitOrder(
        uint32 assetId,
        bool isBuy,
        uint64 limitPx,
        uint64 size,
        bool reduceOnly,
        uint8 encodedTif,
        uint128 cloid,
        address tokenAddress
    ) external onlyOwner {
        _placeLimitOrder(
            assetId,
            isBuy,
            limitPx,
            size,
            reduceOnly,
            encodedTif,
            cloid,
            tokenAddress
        );
    }

    /**
     * @dev Place a limit order with raw values (no decimal formatting)
     * @param assetId The asset ID
     * @param isBuy True to buy, false to sell
     * @param limitPx The limit price (raw 8-decimal format)
     * @param size The size of the order (raw 8-decimal format)
     * @param reduceOnly True to reduce only, false to full order
     * @param encodedTif The time in force encoded as a uint8
     * @param cloid The cloid
     */
    function limitOrderRaw(
        uint32 assetId,
        bool isBuy,
        uint64 limitPx,
        uint64 size,
        bool reduceOnly,
        uint8 encodedTif,
        uint128 cloid
    ) external onlyOwner {
        require(size > 0, "Amount must be greater than 0");

        // Construct the action data for limit order (Action ID 1)
        bytes memory encodedAction = abi.encode(
            assetId, isBuy, limitPx, size, reduceOnly, encodedTif, cloid
        );
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        // Action ID 1 (Limit Order)
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x01;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit LimitOrder(
            assetId, isBuy, limitPx, size, reduceOnly, encodedTif, cloid
        );
    }

    // ========================================
    // Convenience Functions for Common Tokens
    // ========================================

    /**
     * @dev Place a limit sell order to swap asset to USDC
     * @param tokenAddress The token to sell
     * @param amount The amount of tokens to sell (native precision)
     * @param priceUsdc The limit price in USDC (8-decimal format)
     * @param assetId The asset ID for the token/USDC pair
     */
    function sellTokenForUsdc(
        address tokenAddress,
        uint64 amount,
        uint64 priceUsdc,
        uint32 assetId
    ) external onlyOwner {
        _placeLimitOrder(
            assetId,
            false, // sell
            priceUsdc,
            amount,
            false, // not reduce only
            3, // IOC (Immediate or Cancel)
            0, // cloid
            tokenAddress
        );
    }

    /**
     * @dev Place a limit buy order to swap USDC for asset
     * @param tokenAddress The token to buy
     * @param amount The amount of tokens to buy (native precision)
     * @param priceUsdc The limit price in USDC (8-decimal format)
     * @param assetId The asset ID for the token/USDC pair
     */
    function buyTokenWithUsdc(
        address tokenAddress,
        uint64 amount,
        uint64 priceUsdc,
        uint32 assetId
    ) external onlyOwner {
        _placeLimitOrder(
            assetId,
            true, // buy
            priceUsdc,
            amount,
            false, // not reduce only
            3, // IOC (Immediate or Cancel)
            0, // cloid
            tokenAddress
        );
    }

    /**
     * @dev Internal function to place limit order with proper decimal formatting
     */
    function _placeLimitOrder(
        uint32 assetId,
        bool isBuy,
        uint64 limitPx,
        uint64 size,
        bool reduceOnly,
        uint8 encodedTif,
        uint128 cloid,
        address tokenAddress
    ) internal {
        require(size > 0, "Amount must be greater than 0");

        // Apply proper decimal formatting based on token
        uint64 formattedPrice = limitPx;
        uint64 formattedSize = size;

        if (tokenAddress != address(0)) {
            // Format price using HyperLiquid precision rules
            formattedPrice = formatPrice(limitPx, tokenAddress);

            // Truncate size to native precision, then scale to 8-decimal encoding
            uint64 truncatedSize = truncateSize(size, tokenAddress);
            formattedSize = scaleSize(truncatedSize, tokenAddress);
        }

        // Construct the action data for limit order (Action ID 1)
        bytes memory encodedAction = abi.encode(
            assetId,
            isBuy,
            formattedPrice,
            formattedSize,
            reduceOnly,
            encodedTif,
            cloid
        );
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        // Action ID 1 (Limit Order)
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x01;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit LimitOrder(
            assetId,
            isBuy,
            formattedPrice,
            formattedSize,
            reduceOnly,
            encodedTif,
            cloid
        );
    }

    /**
     * @dev Cancel a limit order on the spot market, mostly used for swaping asset to USDC
     * @param assetId The asset ID
     * @param oid The order ID
     */
    function cancelLimitOrder(uint32 assetId, uint64 oid) external onlyOwner {
        require(oid > 0, "Order ID must be greater than 0");

        // Construct the action data for cancel order by oid (Action ID 10)
        bytes memory encodedAction = abi.encode(assetId, oid);
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        // Action ID 10 (Cancel order by oid)
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x0A;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit CancelLimitOrder(assetId, oid);
    }

    /**
     * @dev Swap any token for any other token using the router
     * @param tokenIn The input token address (use address(0) for ETH)
     * @param tokenOut The output token address (use address(0) for ETH)
     * @param amountIn The amount of input tokens to swap
     * @param amountOutMin The minimum amount of output tokens expected
     * @param to The recipient address
     * @param deadline The transaction deadline
     * @param referrer The referrer address for the swap
     */
    function swapTokens(
        address tokenIn,
        address tokenOut,
        uint256 amountIn,
        uint256 amountOutMin,
        address to,
        uint256 deadline,
        address referrer
    ) external payable onlyOwner nonReentrant {
        require(router != address(0), "Router not set");
        require(amountIn > 0, "Amount must be greater than 0");
        require(to != address(0), "Invalid recipient");
        require(deadline >= block.timestamp, "Deadline expired");

        IRouter routerContract = IRouter(router);
        address weth = routerContract.WETH();

        uint256 balanceBefore;
        uint256 balanceAfter;

        if (tokenIn == address(0) && tokenOut != address(0)) {
            // ETH to Token swap
            require(msg.value >= amountIn, "Insufficient ETH sent");

            address[] memory path = new address[](2);
            path[0] = weth;
            path[1] = tokenOut;

            balanceBefore = IERC20(tokenOut).balanceOf(to);

            routerContract.swapExactETHForTokensSupportingFeeOnTransferTokens{
                value: amountIn
            }(amountOutMin, path, to, referrer, deadline);

            balanceAfter = IERC20(tokenOut).balanceOf(to);

            emit TokenSwap(
                tokenIn, tokenOut, amountIn, balanceAfter - balanceBefore, to
            );
        } else if (tokenIn != address(0) && tokenOut == address(0)) {
            // Token to ETH swap
            IERC20(tokenIn).safeTransferFrom(
                msg.sender, address(this), amountIn
            );
            IERC20(tokenIn).approve(router, amountIn);

            address[] memory path = new address[](2);
            path[0] = tokenIn;
            path[1] = weth;

            balanceBefore = to.balance;

            routerContract.swapExactTokensForETHSupportingFeeOnTransferTokens(
                amountIn, amountOutMin, path, to, referrer, deadline
            );

            balanceAfter = to.balance;

            emit TokenSwap(
                tokenIn, tokenOut, amountIn, balanceAfter - balanceBefore, to
            );
        } else if (tokenIn != address(0) && tokenOut != address(0)) {
            // Token to Token swap
            IERC20(tokenIn).safeTransferFrom(
                msg.sender, address(this), amountIn
            );
            IERC20(tokenIn).approve(router, amountIn);

            address[] memory path;

            // Check if we need to route through WETH
            if (tokenIn == weth || tokenOut == weth) {
                path = new address[](2);
                path[0] = tokenIn;
                path[1] = tokenOut;
            } else {
                path = new address[](3);
                path[0] = tokenIn;
                path[1] = weth;
                path[2] = tokenOut;
            }

            balanceBefore = IERC20(tokenOut).balanceOf(to);

            routerContract.swapExactTokensForTokensSupportingFeeOnTransferTokens(
                amountIn, amountOutMin, path, to, referrer, deadline
            );

            balanceAfter = IERC20(tokenOut).balanceOf(to);

            emit TokenSwap(
                tokenIn, tokenOut, amountIn, balanceAfter - balanceBefore, to
            );
        } else {
            revert("Invalid token pair: cannot swap ETH for ETH");
        }
    }

    /**
     * @notice Emergency withdrawal of stuck tokens
     * @dev Only callable by owner in case of emergency
     */
    function emergencyWithdraw(address token, uint256 amount)
        external
        onlyOwner
    {
        if (token == address(0)) {
            payable(owner()).transfer(amount);
        } else {
            IERC20(token).safeTransfer(owner(), amount);
        }
    }

    receive() external payable {}
}
