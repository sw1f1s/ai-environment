export interface Money { cents: number }
export type Amount = Money;
export class Box<T> { constructor(public value: T) {} }
export interface Pricer { price(item: Money): Money }
export class Base { price(item: Money): Money { return item; } }
export class Discount extends Base implements Pricer { price(item: Money): Money { return {cents: item.cents - 1}; } }
export class Receipt { saved!: Box<Money>; }
