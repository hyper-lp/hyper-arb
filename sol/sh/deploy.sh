source config/.env.sol
export pvkey=${DEPLOYER_PV_KEY}
echo "Deploying Vault contract..."
# Fork
forge script script/DeployVault.s.sol:DeployVault --rpc-url http://localhost:8545 --private-key $pvkey
# Tesnet
# forge script script/DeployVault.s.sol:DeployVault --rpc-url https://rpc.hyperliquid-testnet.xyz/evm --private-key $pvkey # --broadcast --

