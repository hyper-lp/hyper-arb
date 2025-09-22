// SPDX-License-Identifier: MIT
pragma solidity ^0.8.19;

import "forge-std/Script.sol";
import "../src/HyperArb.sol";

contract DeployScript is Script {
    function run() external {
        // Load environment variables
        uint256 deployerPrivateKey = vm.envUint("PRIVATE_KEY");
        address hyperSwapRouter = vm.envAddress("HYPERSWAP_ROUTER");
        address projectXRouter = vm.envAddress("PROJECTX_ROUTER");
        
        // Start broadcasting transactions
        vm.startBroadcast(deployerPrivateKey);
        
        // Deploy the Arbitrage contract
        Arbitrage arbitrage = new Arbitrage();
        
        // Set router addresses
        arbitrage.setHyperSwapRouter(hyperSwapRouter);
        arbitrage.setProjectXRouter(projectXRouter);
        
        vm.stopBroadcast();
        
        // Log the deployed contract address explicitly
        console.log("");
        console.log("========================================");
        console.log("ARBITRAGE CONTRACT DEPLOYED");
        console.log("========================================");
        console.log("Contract Address:", address(arbitrage));
        console.log("Owner:", arbitrage.owner());
        console.log("HyperSwap Router:", hyperSwapRouter);
        console.log("ProjectX Router:", projectXRouter);
        console.log("========================================");
        console.log("");
        console.log("Add this address to your config/main.toml:");
        console.log("arbitrage_contract = \"%s\"", address(arbitrage));
        console.log("");
        
        // Save to file for easy copying
        string memory output = string(abi.encodePacked(
            "ARBITRAGE_CONTRACT=", vm.toString(address(arbitrage)), "\n",
            "HYPERSWAP_ROUTER=", vm.toString(hyperSwapRouter), "\n", 
            "PROJECTX_ROUTER=", vm.toString(projectXRouter), "\n",
            "DEPLOYER=", vm.toString(msg.sender), "\n"
        ));
        
        vm.writeFile("./deployed-addresses.txt", output);
        console.log("Addresses saved to: ./deployed-addresses.txt");
    }
}