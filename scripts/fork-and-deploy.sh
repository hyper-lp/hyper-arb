#!/bin/bash

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}   HyperArb Contract Deployment Script ${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# Check if .env exists, if not copy from example
if [ ! -f ../.env ]; then
    echo -e "${YELLOW}No .env file found. Copying from .env.example...${NC}"
    cp ../.env.example ../.env
    echo -e "${YELLOW}Please edit .env file with your configuration and run again.${NC}"
    exit 1
fi

# Load environment variables
source ../.env

# Step 1: Start Anvil fork
echo -e "${GREEN}[1/3] Starting Anvil fork of HyperEVM...${NC}"
echo -e "${YELLOW}RPC URL: $RPC_URL${NC}"

# Kill any existing anvil process
pkill anvil 2>/dev/null

# Start Anvil in background with fork
anvil \
    --fork-url $RPC_URL \
    --fork-block-number $FORK_BLOCK_NUMBER \
    --port 8545 \
    --accounts 10 \
    --balance 10000 \
    --block-time 2 \
    &

ANVIL_PID=$!
echo -e "${GREEN}Anvil started with PID: $ANVIL_PID${NC}"

# Wait for Anvil to be ready
echo -e "${YELLOW}Waiting for Anvil to be ready...${NC}"
sleep 5

# Step 2: Build contracts
echo -e "${GREEN}[2/3] Building contracts...${NC}"
cd ../sol
forge build

if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to build contracts${NC}"
    kill $ANVIL_PID
    exit 1
fi

# Step 3: Deploy contract
echo -e "${GREEN}[3/3] Deploying Arbitrage contract...${NC}"
forge script script/Deploy.s.sol:DeployScript \
    --rpc-url http://localhost:8545 \
    --broadcast \
    -vvv

if [ $? -ne 0 ]; then
    echo -e "${RED}Failed to deploy contract${NC}"
    kill $ANVIL_PID
    exit 1
fi

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}   DEPLOYMENT SUCCESSFUL!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "${YELLOW}Contract addresses saved to: ./deployed-addresses.txt${NC}"
echo ""
echo -e "${BLUE}Next steps:${NC}"
echo -e "1. Copy the contract address from above"
echo -e "2. Add it to your config/main.toml file:"
echo -e "   ${YELLOW}arbitrage_contract = \"<CONTRACT_ADDRESS>\"${NC}"
echo -e "3. Update your arbitrager.rs to use this contract"
echo ""
echo -e "${GREEN}Anvil is running in the background (PID: $ANVIL_PID)${NC}"
echo -e "${YELLOW}To stop Anvil: kill $ANVIL_PID${NC}"
echo ""

# Keep script running to show logs
echo -e "${BLUE}Press Ctrl+C to stop Anvil and exit${NC}"
wait $ANVIL_PID