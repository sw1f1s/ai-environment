import {Money, Amount, Pricer} from './types';
export function refund(amount: number): number;
export function refund(amount: Money): Money;
export function refund(amount: number | Money): number | Money { return amount; }
export function run(pricer: Pricer, amount: Amount): Money {
  const first = refund(3);
  const second = refund(amount);
  return pricer.price(second);
}
export function decoy(value: string): string { return value; }
