/// =============================================================================
/// Unichain Execution Strategy
/// =============================================================================
///
/// @description: Unichain execution strategy optimized for Unichain network.
/// Unichain provides advanced transaction features and optimizations for
/// high-frequency trading and market making operations.
/// @reference: https://docs.unichain.org/docs/technical-information/advanced-txn
/// =============================================================================
use async_trait::async_trait;

use crate::maker::exec::ExecStrategyName;

use super::super::ExecStrategy;

/// =============================================================================
/// @struct: UnichainExec
/// @description: Unichain execution strategy implementation
/// @behavior: Optimized for Unichain network with advanced transaction features
/// =============================================================================
pub struct UnichainExec;

/// =============================================================================
/// @function: new
/// @description: Create a new Unichain execution strategy instance
/// @return Self: New UnichainExec instance
/// =============================================================================
impl Default for UnichainExec {
    fn default() -> Self {
        Self::new()
    }
}

impl UnichainExec {
    pub fn new() -> Self {
        Self
    }
}

/// =============================================================================
/// TRAIT IMPLEMENTATION: ExecStrategy
/// =============================================================================
/// OVERRIDDEN FUNCTIONS:
/// - name(): Returns "Unichain_Strategy"
/// 
/// INHERITED FUNCTIONS (using default implementation):
/// - pre_hook(): Default logging
/// - post_hook(): Default event publishing
/// - execute(): Default orchestration flow
/// - simulate(): Default EVM simulation
/// - broadcast(): Default mempool broadcast (will be customized for Unichain features)
/// 
/// TODO: Implement custom functions for Unichain advanced features:
/// - Override broadcast() for Unichain-specific transaction features
/// - Leverage high-frequency trading optimizations
/// - Implement market maker specific enhancements
/// @reference: https://docs.unichain.org/docs/technical-information/advanced-txn
/// =============================================================================
#[async_trait]
impl ExecStrategy for UnichainExec {
    /// OVERRIDDEN: Custom strategy name
    fn name(&self) -> String {
        ExecStrategyName::UnichainStrategy.as_str().to_string()
    }
    
    // TODO: Override broadcast() for Unichain advanced transaction features
    // async fn broadcast(&self, prepared: Vec<Trade>, mmc: MarketMakerConfig, env: EnvConfig) -> Result<Vec<BroadcastData>, String>
}
