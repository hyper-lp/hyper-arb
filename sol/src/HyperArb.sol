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

    address public constant CORE_WRITER = 0x3333333333333333333333333333333333333333;

    address public router;

    event BridgeToCore(address token, uint256 amount);

    event BridgeToEvm(uint64 token, uint64 weiAmount);

    event CrossMarketTransfer(uint256 ntl, bool toPerp);

    event LimitOrder(
        uint32 assetId, bool isBuy, uint64 limitPx, uint64 size, bool reduceOnly, uint8 encodedTif, uint128 cloid
    );

    event APIWalletAdded(address indexed walletAddress, string indexed walletName);

    event SpotTransfer(address indexed to, uint64 indexed token, uint64 indexed weiAmount);

    event TokenSwap(
        address indexed tokenIn,
        address indexed tokenOut,
        uint256 amountIn,
        uint256 amountOut,
        address indexed to
    );

    event SystemAddressUpdated(address indexed oldAddress, address indexed newAddress);
    event RouterUpdated(address indexed oldRouter, address indexed newRouter);
    event KeeperUpdated(address indexed oldKeeper, address indexed newKeeper);
    event CancelLimitOrder(uint32 assetId, uint64 oid);
    // ========================================
    // Constructor
    // ========================================

    constructor()
        Ownable(msg.sender)
    {}

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
    function addApiWallet(address walletAddress, string memory walletName) external onlyOwner {
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
    function bridgeToEvm(address tokenSystemAddress, uint64 token, uint64 weiAmount) external onlyOwner {
        require(weiAmount > 0, "Amount must be greater than 0");

        // Construct the action data for spot transfer (Action ID 5)
        bytes memory encodedAction = abi.encode(tokenSystemAddress, token, weiAmount);
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
    function spotTransfer(address to, uint64 token, uint64 weiAmount) external onlyOwner {
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
    function bridgeToCore(address tokenSystemAddress, address token, uint256 amount) external onlyOwner {
        require(amount > 0, "Amount must be greater than 0");

        IERC20(token).transfer(tokenSystemAddress, amount);

        emit BridgeToCore(token, amount);
    }

    /**
     * @dev Place a limit order on the spot market, mostly used for swaping asset to USDC
     * @param assetId The asset ID
     * @param isBuy True to buy, false to sell
     * @param limitPx The limit price
     * @param size The size of the order
     * @param reduceOnly True to reduce only, false to full order
     * @param encodedTif The time in force encoded as a uint8
     * @param cloid The cloid
     */
    function limitOrder(
        uint32 assetId,
        bool isBuy,
        uint64 limitPx,
        uint64 size,
        bool reduceOnly,
        uint8 encodedTif,
        uint128 cloid
    ) external onlyOwner {
        require(size > 0, "Amount must be greater than 0");

        // Construct the action data for USD class transfer (Action ID 7)
        bytes memory encodedAction = abi.encode(assetId, isBuy, limitPx, size, reduceOnly, encodedTif, cloid);
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        // Action ID 7 (USD class transfer)
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x01;

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit LimitOrder(assetId, isBuy, limitPx, size, reduceOnly, encodedTif, cloid);
    }

    /**
     * @dev Cancel a limit order on the spot market, mostly used for swaping asset to USDC
     * @param assetId The asset ID
     * @param oid The order ID
     */
    function cancelLimitOrder(
        uint32 assetId,
        uint64 oid
    ) external onlyOwner {
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
     * @dev Cancel a limit order by client order ID
     * @param assetId The asset ID
     * @param cloid The client order ID to cancel
     */
    function cancelLimitOrderByCloid(
        uint32 assetId,
        uint128 cloid
    ) external onlyOwner {
        require(cloid > 0, "Client order ID must be greater than 0");

        // Construct the action data for cancel order by cloid (Action ID 11)
        bytes memory encodedAction = abi.encode(assetId, cloid);
        bytes memory data = new bytes(4 + encodedAction.length);

        // Version 1
        data[0] = 0x01;
        // Action ID 11 (Cancel order by cloid)
        data[1] = 0x00;
        data[2] = 0x00;
        data[3] = 0x0B; // 0x0B = 11 in decimal

        // Copy encoded action data
        for (uint256 i = 0; i < encodedAction.length; i++) {
            data[4 + i] = encodedAction[i];
        }

        ICoreWriter(CORE_WRITER).sendRawAction(data);

        emit CancelLimitOrderByCloid(assetId, cloid);
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
    ) external onlyOwner nonReentrant payable {
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
            
            routerContract.swapExactETHForTokensSupportingFeeOnTransferTokens{value: amountIn}(
                amountOutMin,
                path,
                to,
                referrer,
                deadline
            );
            
            balanceAfter = IERC20(tokenOut).balanceOf(to);
            
            emit TokenSwap(tokenIn, tokenOut, amountIn, balanceAfter - balanceBefore, to);
            
        } else if (tokenIn != address(0) && tokenOut == address(0)) {
            // Token to ETH swap
            IERC20(tokenIn).safeTransferFrom(msg.sender, address(this), amountIn);
            IERC20(tokenIn).approve(router, amountIn);
            
            address[] memory path = new address[](2);
            path[0] = tokenIn;
            path[1] = weth;
            
            balanceBefore = to.balance;
            
            routerContract.swapExactTokensForETHSupportingFeeOnTransferTokens(
                amountIn,
                amountOutMin,
                path,
                to,
                referrer,
                deadline
            );
            
            balanceAfter = to.balance;
            
            emit TokenSwap(tokenIn, tokenOut, amountIn, balanceAfter - balanceBefore, to);
            
        } else if (tokenIn != address(0) && tokenOut != address(0)) {
            // Token to Token swap
            IERC20(tokenIn).safeTransferFrom(msg.sender, address(this), amountIn);
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
                amountIn,
                amountOutMin,
                path,
                to,
                referrer,
                deadline
            );
            
            balanceAfter = IERC20(tokenOut).balanceOf(to);
            
            emit TokenSwap(tokenIn, tokenOut, amountIn, balanceAfter - balanceBefore, to);
            
        } else {
            revert("Invalid token pair: cannot swap ETH for ETH");
        }
    }

    /**
     * @notice Emergency withdrawal of stuck tokens
     * @dev Only callable by owner in case of emergency
     */
    function emergencyWithdraw(address token, uint256 amount) external onlyOwner {
        if (token == address(0)) {
            payable(owner()).transfer(amount);
        } else {
            IERC20(token).safeTransfer(owner(), amount);
        }
    }

    receive() external payable {}
}
