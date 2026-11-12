use crate::types::Money;
pub fn refund(value: Money) -> i64 { value.amount }
pub fn identity<T>(value: T) -> T { value }
