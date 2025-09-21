Implementation

A multi-phase cross-chain arbitrage system leveraging HyperCore infrastructure. Phase 1 implements statistical arbitrage by detecting price spreads
  between EVM pools (Hyperswap, ProjectX) and reference prices from oracles (Pyth, RedStone) or HyperCore precompiles, executing trades when spreads
  exceed threshold basis points. Phase 2 adds automated inventory rebalancing via lending protocols or Corewriter to neutralize volatile positions.
  Phase 3 delivers a Neon-based frontend. Architecture uses Rust for high-performance arbitrage execution, TypeScript for rebalancing logic, Docker
  Compose for deployment, and Redis for state management. The system stops trading when inventory becomes imbalanced, waiting for the rebalancer to
  restore equilibrium.

Reference
- HyperCore via Precompile or API
- Oracles (Pyth et RedStone)

Onchain
- Hyperswap
- ProjectX

Ideas:
- Backtest : voir avec les services d’historique de prix : https://www.hypedexer.com/
- Neutralise the volatile part of the inventory on HC
- Benchmark CBB !

Phase 1 : statistical arbitrage : buy/sell on EVM when spread > x bps versus 1 reference price (but multiple instance with different combo of AMM/oracle) : single side inventory (idea: correlation heatmap)
- Merso
- Stop when condition is inventory ratio imbalanced, wait the rebalancing keeper to operate

Phase 2 : inventory rebalancing on top of Corewriter or via lending borrow : Typescript

Phase 3 : frontend with Neon

Library/Component
Read pool price
Swap pools
Read reference price (precompile, APIs, oracles)
Compute spreads

Docker Compose 
Arb keeper: Rust
Rebalance Keeper: Typescript
Neon/Redis
VM to deploy the compose stack
UI: Neon x Vercel me@fberger.xyz

