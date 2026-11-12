pub struct Money { pub amount: i64 }
pub type Price = Money;
pub struct Basket { pub values: Option<Money> }
pub trait Pricing { fn price(&self) -> Money; }
pub struct Shop;
impl Pricing for Shop { fn price(&self) -> Money { Money { amount: 7 } } }
