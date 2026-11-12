pub mod types;
pub mod ledger;
mod decoy { pub fn refund(value: i64) -> i64 { value + 99 } }
use types::{Shop, Pricing};
use ledger::{refund, identity};
const LABEL: &str = "🦀 память";
pub fn run(shop: Shop) -> i64 { identity(refund(shop.price())) }
