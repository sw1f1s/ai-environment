// Межфайловая проверка — 雪 😀
namespace Warehouse;
public static class Engine {
    public static Money Refund(int stock, int allocated) => new Money(stock + allocated);
    public static Money Refund(Money stock, Money allocated) => Refund(stock.Cents, allocated.Cents);
    public static Money Quote(IPricer pricer, int units) => pricer.Price(units);
    public static Money Run() {
        var first = Refund(8, 2);
        var second = Refund(new Money(5), first);
        IPricer pricer = new DiscountPricer();
        var box = new Box<Money>(Quote(pricer, second.Cents));
        return box.Value;
    }
}
