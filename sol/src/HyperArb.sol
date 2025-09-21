// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "@openzeppelin/contracts/token/ERC20/ERC20.sol";
import "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import "@openzeppelin/contracts/access/Ownable.sol";
import "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {ICoreWriter} from "./interfaces/ICore.sol";

/**
 * @title Arbitrage
 * @notice Arbitrage contract for HyperLP Protocol
 */
contract Arbitrage is ERC20, Ownable, ReentrancyGuard {
    using SafeERC20 for IERC20;

    // ========================================
    // State Variables
    // ========================================

    address public constant CORE_WRITER = 0x3333333333333333333333333333333333333333;

    address public systemAddress;
    address public keeper;

    event BridgeToCore(address token, uint256 amount);

    event BridgeToEvm(uint64 token, uint64 weiAmount);

    event CrossMarketTransfer(uint256 ntl, bool toPerp);

    event LimitOrder(
        uint32 assetId, bool isBuy, uint64 limitPx, uint64 size, bool reduceOnly, uint8 encodedTif, uint128 cloid
    );

    event APIWalletAdded(address indexed walletAddress, string indexed walletName);

    event SpotTransfer(address indexed to, uint64 indexed token, uint64 indexed weiAmount);
    // ========================================
    // Constructor
    // ========================================

    constructor(string memory _name, string memory _symbol, address _systemAddress)
        ERC20(_name, _symbol)
        Ownable(msg.sender)
    {
        systemAddress = _systemAddress;
    }

    // ========================================
    // Admin Functions
    // ========================================

    function setSystemAddress(address _systemAddress) external onlyOwner {
        require(_systemAddress != address(0), "Invalid system address");
        systemAddress = _systemAddress;
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
}
