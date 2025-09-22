source config/.env.sol
export pvkey=${DEPLOYER_PV_KEY}
echo "Deploying Vault contract..."
# Fork
forge script script/DeployVault.s.sol:DeployVault --rpc-url http://localhost:8545 --private-key $pvkey
# Testnet
# forge script script/DeployVault.s.sol:DeployVault --rpc-url https://rpc.hyperliquid-testnet.xyz/evm --private-key $pvkey # --broadcast --
# forge script script/DeployHyperArb.s.sol:DeployHyperArbScript --rpc-url https://rpc.hyperliquid.xyz/evm --private-key PRIVATE_KEY --broadcast
