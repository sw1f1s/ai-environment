#pragma once
namespace warehouse {
struct Money { int cents; };
using Amount = Money;
struct Receipt { Amount total; };
class Store {
public:
    virtual Money price(int units) const;
};
class DiscountStore : public Store {
public:
    Money price(int units) const override;
};
Money refund(int stock, int allocated);
Money refund(Money stock, Money allocated);
Money reserve(const Store &store, int units);
Money run();
template<class T> T identity(T value) { return value; }
}
