using Amount = Warehouse.Money;
namespace Warehouse;
public readonly struct Money {
    public int Cents { get; }
    public Money(int cents) { Cents = cents; }
}
public sealed class Box<T> {
    public T Value;
    public Box(T value) { Value = value; }
}
public interface IPricer { Money Price(int units); }
public class BasePricer : IPricer {
    public virtual Money Price(int units) => new Money(units * 5);
}
public sealed class DiscountPricer : BasePricer {
    public override Money Price(int units) => new Money(units * 3);
}
public sealed class Receipt {
    public Amount Total;
    public Box<Money> Saved = new Box<Money>(new Money(0));
}
