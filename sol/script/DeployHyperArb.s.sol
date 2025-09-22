// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import {Script, console} from "forge-std/Script.sol";
import {Arbitrage} from "../src/HyperArb.sol";

contract DeployHyperArbScript is Script {
    Arbitrage public arbitrage;
    
    // Configuration parameters - update these for your deployment
    struct DeployConfig {
        address router;
        address owner;
    }
    
    function setUp() public {}

    function run() public {
        // Load deployment configuration
        DeployConfig memory config = getDeployConfig();
        
        console.log("Deploying HyperArb with the following configuration:");
        console.log("Router Address:", config.router);
        console.log("Owner Address:", config.owner);
        
        vm.startBroadcast();

        // Deploy the Arbitrage contract
        arbitrage = new Arbitrage();
        
        console.log("HyperArb deployed at:", address(arbitrage));
        
        // Configure the contract if router address is provided
        if (config.router != address(0)) {
            arbitrage.setRouter(config.router);
            console.log("Router configured:", config.router);
        }
        
        // Transfer ownership if a different owner is specified
        if (config.owner != address(0) && config.owner != msg.sender) {
            arbitrage.transferOwnership(config.owner);
            console.log("Ownership transferred to:", config.owner);
        }

        vm.stopBroadcast();
        
        // Log deployment summary
        console.log("\n=== DEPLOYMENT SUMMARY ===");
        console.log("Contract Address:", address(arbitrage));
        console.log("Owner:", arbitrage.owner());
        console.log("Router Address:", arbitrage.router());
        console.log("Core Writer:", arbitrage.CORE_WRITER());
        
        // Verify contract state
        verifyDeployment(config);
    }
    
    function getDeployConfig() internal view returns (DeployConfig memory) {
        // Try to read from environment variables first
        address router = vm.envOr("ROUTER_ADDRESS", address(0));
        address owner = vm.envOr("OWNER_ADDRESS", address(0));

        return DeployConfig({
            router: router,
            owner: owner
        });
    }
    
    function verifyDeployment(DeployConfig memory config) internal view {
        console.log("\n=== DEPLOYMENT VERIFICATION ===");
        
        // Verify contract deployment
        require(address(arbitrage) != address(0), "Contract not deployed");
        console.log("Contract deployed successfully");
        
        // Verify router if provided
        if (config.router != address(0)) {
            require(arbitrage.router() == config.router, "Router address mismatch");
            console.log("Router configured correctly");
        }
        
        // Verify ownership
        address expectedOwner = config.owner != address(0) ? config.owner : msg.sender;
        require(arbitrage.owner() == expectedOwner, "Owner mismatch");
        console.log("Ownership configured correctly");
        
        console.log("All verifications passed!");
    }
    
    // Helper function to deploy with specific parameters (for testing)
    function deployWithParams(
        address _router,
        address _owner
    ) public returns (address) {
        vm.startBroadcast();
        
        arbitrage = new Arbitrage();
        
        if (_router != address(0)) {
            arbitrage.setRouter(_router);
        }
        
        if (_owner != address(0) && _owner != msg.sender) {
            arbitrage.transferOwnership(_owner);
        }
        
        vm.stopBroadcast();
        
        return address(arbitrage);
    }
} 