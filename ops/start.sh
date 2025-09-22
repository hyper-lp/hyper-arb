#!/bin/bash

RED='\033[0;31m'
NC='\033[0m'

# --- Usage ---
# Requires Rust and Cargo to be installed.
# sh ops/start.sh

export PROGRAM=$1

function start() {
    trap '' SIGINT

    # source config/.env
    # export WALLET_PUB_KEYS=WALLET_PUB_KEYS
    # export WALLET_PRIVATE_KEYS=WALLET_PRIVATE_KEYS

    # ------------- Execute -------------
    if [ "$1" = "test" ]; then
        cargo test -- --nocapture
    else
        echo "Building program ..."
        cargo build --bin $1 -q 2>/dev/null
        echo "Build successful. Executing..."
        (
            trap - SIGINT
            export RUST_LOG="off,shd=trace,$PROGRAM=trace,demo=trace,test=trace"
            cargo run --bin $1 -q # 2>/dev/null
        )
        echo "Program has finished or was interrupted. Continuing with the rest of the shell script ..."
        status+=($?)
        if [ $status -ne 0 ]; then
            echo "Error: $status on program ${RED}${program}${NC}"
            exit 1
        fi
    fi
}

start $1

