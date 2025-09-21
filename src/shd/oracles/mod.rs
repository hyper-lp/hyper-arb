// Oracles and external data fetching

pub mod hypercore;
pub mod pyth;
pub mod redstone;

pub use hypercore::Hypercore;
pub use pyth::{Pyth, PythPriceIds, fetch_hype_usd_price};
pub use redstone::{Redstone, fetch_btc_usd_price, fetch_eth_usd_price};
