// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {ICoreWriter} from "./interfaces/ICore.sol";
import {IHyperSwapRouter, IProjectXRouter} from "./interfaces/ISwapRouter.sol";

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

    address public hyperSwapRouter;
    address public projectXRouter;
    bool public paused;

    // Structs for arbitrage parameters
    struct PoolSwapParams {
        string dex;
        address routerAddress;
        address tokenIn;
        address tokenOut;
        uint256 amountIn;
        uint256 amountOutMin;
        string poolAddress;
        uint24 poolFeeTier;
        address recipient;
    }

    struct SpotOrderParams {
        string baseToken;
        string quoteToken;
        bool isBuy;
        uint256 amount; // Amount in native token units (e.g., 18 decimals for ERC20)
        uint256 price; // Raw spot price in 8 decimal format
        uint8 szDecimals; // Token's sz decimals from Hyperliquid
        uint8 weiDecimals; // Token's wei decimals from Hyperliquid
    }

    struct DoubleLegOpportunity {
        uint256 amountInBuy;
        uint256 amountInSell;
        uint256 expectedProfitUsd;
        uint256 gasCostUsd;
    }

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

<<<<<<< HEAD
    event SystemAddressUpdated(
        address indexed oldAddress, address indexed newAddress
    );
    event RouterUpdated(address indexed oldRouter, address indexed newRouter);
=======
    event SystemAddressUpdated(address indexed oldAddress, address indexed newAddress);
    event HyperSwapRouterUpdated(address indexed oldRouter, address indexed newRouter);
    event ProjectXRouterUpdated(address indexed oldRouter, address indexed newRouter);
    event ArbitrageExecuted(
        string dex, address tokenIn, address tokenOut, uint256 amountIn, uint256 amountOut, uint256 expectedProfit
    );
>>>>>>> c6bfdda (feat(Bundle-Arb-via-corewriter-spot-exec-+-pool-exec): None)
    event KeeperUpdated(address indexed oldKeeper, address indexed newKeeper);
    event CancelLimitOrder(uint32 assetId, uint64 oid);
    event Paused(address account);
    event Unpaused(address account);
    // ========================================
    // Constructor
    // ========================================

    constructor() Ownable(msg.sender) {
<<<<<<< HEAD
        initializeTokenDecimals();
=======
        paused = false;
    }

    modifier whenNotPaused() {
        require(!paused, "Contract is paused");
        _;
>>>>>>> c6bfdda (feat(Bundle-Arb-via-corewriter-spot-exec-+-pool-exec): None)
    }

    // ========================================
    // Admin Functions
    // ========================================

    function setPaused(bool _paused) external onlyOwner {
        paused = _paused;
        if (_paused) {
            emit Paused(msg.sender);
        } else {
            emit Unpaused(msg.sender);
        }
    }

    function setHyperSwapRouter(address _router) external onlyOwner {
        require(_router != address(0), "Invalid router address");
        address oldRouter = hyperSwapRouter;
        hyperSwapRouter = _router;
        emit HyperSwapRouterUpdated(oldRouter, _router);
    }

    function setProjectXRouter(address _router) external onlyOwner {
        require(_router != address(0), "Invalid router address");
        address oldRouter = projectXRouter;
        projectXRouter = _router;
        emit ProjectXRouterUpdated(oldRouter, _router);
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
     * @dev Execute Uniswap V3 exactInputSingle swap
     * @param dex The DEX to use ("hyperswap" or "projectx")
     * @param tokenIn The input token address
     * @param tokenOut The output token address
     * @param fee The pool fee tier (500, 3000, 10000 for 0.05%, 0.3%, 1%)
     * @param amountIn The amount of input tokens to swap
     * @param amountOutMinimum The minimum amount of output tokens expected
     * @param recipient The recipient address
     * @param sqrtPriceLimitX96 The price limit (0 for no limit)
     */
    function exactInputSingle(
        string memory dex,
        address tokenIn,
        address tokenOut,
        uint24 fee,
        uint256 amountIn,
        uint256 amountOutMinimum,
        address recipient,
        uint160 sqrtPriceLimitX96
    ) public onlyOwner nonReentrant whenNotPaused returns (uint256 amountOut) {
        require(amountIn > 0, "Amount must be greater than 0");
        require(recipient != address(0), "Invalid recipient");

        // Transfer tokens from sender and approve router
        IERC20(tokenIn).safeTransferFrom(msg.sender, address(this), amountIn);

        if (keccak256(bytes(dex)) == keccak256(bytes("hyperswap"))) {
            require(hyperSwapRouter != address(0), "HyperSwap router not set");
            IERC20(tokenIn).approve(hyperSwapRouter, amountIn);

            IHyperSwapRouter.ExactInputSingleParams memory params = IHyperSwapRouter.ExactInputSingleParams({
                tokenIn: tokenIn,
                tokenOut: tokenOut,
                fee: fee,
                recipient: recipient,
                amountIn: amountIn,
                amountOutMinimum: amountOutMinimum,
                sqrtPriceLimitX96: sqrtPriceLimitX96
            });

            amountOut = IHyperSwapRouter(hyperSwapRouter).exactInputSingle(params);
        } else if (keccak256(bytes(dex)) == keccak256(bytes("projectx"))) {
            require(projectXRouter != address(0), "ProjectX router not set");
            IERC20(tokenIn).approve(projectXRouter, amountIn);

            IProjectXRouter.ExactInputSingleParams memory params = IProjectXRouter.ExactInputSingleParams({
                tokenIn: tokenIn,
                tokenOut: tokenOut,
                fee: fee,
                recipient: recipient,
                deadline: block.timestamp + 300, // 5 minutes deadline for ProjectX
                amountIn: amountIn,
                amountOutMinimum: amountOutMinimum,
                sqrtPriceLimitX96: sqrtPriceLimitX96
            });

<<<<<<< HEAD
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
=======
            amountOut = IProjectXRouter(projectXRouter).exactInputSingle(params);
>>>>>>> c6bfdda (feat(Bundle-Arb-via-corewriter-spot-exec-+-pool-exec): None)
        } else {
            revert("Invalid DEX: must be 'hyperswap' or 'projectx'");
        }

        emit TokenSwap(tokenIn, tokenOut, amountIn, amountOut, recipient);
        return amountOut;
    }

    /**
     * @dev Execute double-leg arbitrage: EVM DEX swap + Core spot order
     * @param poolSwapParams Parameters for the DEX pool swap
     * @param spotOrderParams Parameters for the Core spot order (with precise decimals)
     * @param opportunity Information about the arbitrage opportunity
     */
    function evmCoreArb(
        PoolSwapParams calldata poolSwapParams,
        SpotOrderParams calldata spotOrderParams,
        DoubleLegOpportunity calldata opportunity
    ) external onlyOwner nonReentrant whenNotPaused {
        // Validate parameters
        require(poolSwapParams.amountIn > 0, "Invalid pool swap amount");
        require(spotOrderParams.amount > 0, "Invalid spot order amount");
        require(opportunity.expectedProfitUsd > opportunity.gasCostUsd, "No profit after gas");
        
        // Validate decimal parameters to prevent underflow
        require(spotOrderParams.weiDecimals >= spotOrderParams.szDecimals, "Invalid decimal configuration");
        require(spotOrderParams.szDecimals <= 8, "szDecimals too large");
        require(spotOrderParams.weiDecimals <= 18, "weiDecimals too large");
        
        // Check balance before swap
        uint256 tokenInBalance = IERC20(poolSwapParams.tokenIn).balanceOf(address(this));
        require(tokenInBalance >= poolSwapParams.amountIn, "Insufficient token balance for swap");

        // Step 1: Execute DEX swap (buy leg) - Quote to Base
        uint256 balanceBefore = IERC20(poolSwapParams.tokenOut).balanceOf(poolSwapParams.recipient);
        
        uint256 baseTokensReceived = exactInputSingle(
            poolSwapParams.dex,
            poolSwapParams.tokenIn,
            poolSwapParams.tokenOut,
            poolSwapParams.poolFeeTier,
            poolSwapParams.amountIn,
            poolSwapParams.amountOutMin,
            poolSwapParams.recipient,
            0 // No price limit
        );
        
        // Verify swap success
        uint256 balanceAfter = IERC20(poolSwapParams.tokenOut).balanceOf(poolSwapParams.recipient);
        require(balanceAfter > balanceBefore, "DEX swap failed");
        require(baseTokensReceived >= poolSwapParams.amountOutMin, "Insufficient output from swap");

        // Step 2: Get asset ID for the spot order
        // AssetID = SpotIndex + 10000 (from HLConversions.spotToAssetId)
        // For simplicity, using hardcoded mappings (should be dynamic in production)
        uint32 assetId = getAssetId(spotOrderParams.baseToken);

        // Step 3: Calculate spot order parameters following TypeScript CoreWriterUtils logic
        uint64 sz;
        uint64 limitPx;

        if (!spotOrderParams.isBuy) {
            // SELL: Match swapAssetToUsdc logic from CoreWriterUtils.ts

            // Convert EVM amount to Core wei format
            // For ERC20 tokens with 18 decimals and evmExtraWeiDecimals=10 (like WHYPE):
            // coreWei = evmAmount / 10^evmExtraWeiDecimals
            uint256 coreWei = spotOrderParams.amount / (10 ** 10); // evmExtraWeiDecimals=10 for WHYPE

            // Convert Core wei to sz units: weiToSz logic
            // sz = coreWei / 10^(weiDecimals - szDecimals)
            uint256 divisor = 10 ** (spotOrderParams.weiDecimals - spotOrderParams.szDecimals);
            uint256 szNative = coreWei / divisor;

            // Apply truncation (Python logic: truncate(float(balance), sz_decimals))
            uint256 szTruncated = truncateToPrecision(szNative, spotOrderParams.szDecimals);

            // Scale to 8 decimals (order encoding expects 8-decimal format)
            uint256 scaleExponent = 8 - spotOrderParams.szDecimals;
            sz = uint64(szTruncated * (10 ** scaleExponent));

            // Format price: for sell orders apply 1% slippage (99% of price)
            limitPx = formatSpotPrice(
                (spotOrderParams.price * 99) / 100, spotOrderParams.szDecimals, spotOrderParams.baseToken
            );
        } else {
            // BUY: Match swapUsdcToAsset logic from CoreWriterUtils.ts

            // Convert USDC amount from EVM (6 decimals) to spot format (8 decimals)
            uint256 usdcSpot = spotOrderParams.amount * (10 ** 2);

            // Calculate sz directly: sz = usdcSpot / spotPx
            uint256 szNative = usdcSpot / spotOrderParams.price;

            // Apply truncation
            uint256 szTruncated = truncateToPrecision(szNative, spotOrderParams.szDecimals);

            // Scale to 8 decimals
            uint256 scaleExponent = 8 - spotOrderParams.szDecimals;
            sz = uint64(szTruncated * (10 ** scaleExponent));

            // Format price: for buy orders apply 1% slippage (101% of price)
            limitPx = formatSpotPrice(
                (spotOrderParams.price * 101) / 100, spotOrderParams.szDecimals, spotOrderParams.baseToken
            );
        }

        // Step 4: Place spot limit order on Hyperliquid
        // Following TypeScript: use IOC (Immediate or Cancel) for swaps
        limitOrder(
            assetId,
            spotOrderParams.isBuy,
            limitPx,
            sz,
            false, // Not reduce only
            3, // TIF: IOC (Immediate or Cancel) - from LIMIT_ORDER_TIF_IOC
            0 // cloid: 0 (matching TypeScript implementation)
        );

        emit ArbitrageExecuted(
            poolSwapParams.dex,
            poolSwapParams.tokenIn,
            poolSwapParams.tokenOut,
            poolSwapParams.amountIn,
            baseTokensReceived,
            opportunity.expectedProfitUsd
        );
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

    /**
     * @dev Truncate a number to specified decimal places (Python logic)
     * Mimics: Math.floor(number * factor) / factor where factor = 10^decimals
     */
    function truncateToPrecision(uint256 number, uint8 decimals) internal pure returns (uint256) {
        if (decimals == 0) return number;
        
        // Implement actual truncation: Math.floor(number * factor) / factor
        // Since we're working with integers representing fixed-point numbers,
        // we need to remove the least significant digits beyond our precision
        uint256 factor = 10 ** uint256(decimals);
        
        // This truncates by dividing by factor and then multiplying back
        // Integer division naturally floors the result
        return (number / factor) * factor;
    }

    /**
     * @dev Format spot price following TypeScript formatPrice logic
     * Applies Hyperliquid precision rules: max (8 - szDecimals) decimal places
     */
    function formatSpotPrice(uint256 price, uint8 szDecimals, string memory tokenSymbol)
        internal
        pure
        returns (uint64)
    {
        bytes32 symbolHash = keccak256(bytes(tokenSymbol));
        uint256 formattedPrice = price;
        
        // Apply significant digits rounding based on token type
        if (symbolHash == keccak256(bytes("BTC"))) {
            // BTC: 6 significant digits
            formattedPrice = roundToSignificantDigits(price, 6);
        } else if (symbolHash == keccak256(bytes("ETH")) || symbolHash == keccak256(bytes("USDT"))) {
            // ETH/USDT: 5 significant digits
            formattedPrice = roundToSignificantDigits(price, 5);
        } else {
            // Other tokens: Round to 8 - szDecimals decimal places
            uint8 maxDecimals = szDecimals < 8 ? 8 - szDecimals : 0;
            if (maxDecimals > 0) {
                // Round to maxDecimals places
                uint256 divisor = 10 ** (8 - maxDecimals);
                formattedPrice = (price / divisor) * divisor;
            }
        }
        
        require(formattedPrice <= type(uint64).max, "Price overflow");
        return uint64(formattedPrice);
    }
    
    /**
     * @dev Round a price to N significant digits
     * @param price Price in 8 decimal format
     * @param sigDigits Number of significant digits to keep
     */
    function roundToSignificantDigits(uint256 price, uint8 sigDigits) internal pure returns (uint256) {
        if (price == 0) return 0;
        
        // Find the magnitude (number of digits)
        uint256 magnitude = 0;
        uint256 temp = price;
        while (temp >= 10) {
            temp /= 10;
            magnitude++;
        }
        
        // Calculate how many digits to remove
        if (magnitude >= sigDigits) {
            uint256 digitsToRemove = magnitude - sigDigits + 1;
            uint256 divisor = 10 ** digitsToRemove;
            // Round by dividing and multiplying back
            return (price / divisor) * divisor;
        }
        
        return price;
    }

    /**
     * @dev Get asset ID for a token (SpotIndex + 10000)
     * In production, this should query the actual spot index
     */
    function getAssetId(string memory tokenSymbol) internal pure returns (uint32) {
        bytes32 symbolHash = keccak256(bytes(tokenSymbol));

        // These would need to be dynamically fetched in production
        // Using example spot indices + 10000
        if (symbolHash == keccak256(bytes("WHYPE")) || symbolHash == keccak256(bytes("HYPE"))) {
            return 10150; // Assuming spot index 150 for HYPE/USDC
        } else if (symbolHash == keccak256(bytes("BTC"))) {
            return 10000; // Assuming spot index 0 for BTC/USDC
        } else if (symbolHash == keccak256(bytes("ETH"))) {
            return 10001; // Assuming spot index 1 for ETH/USDC
        } else {
            revert("Unsupported token for spot order");
        }
    }

    receive() external payable {}
}
