#include "warehouse.hpp"
namespace warehouse {
Money Store::price(int units) const { return Money{units * 5}; }
Money DiscountStore::price(int units) const { return Money{units * 3}; }
Money refund(int stock, int allocated) { return Money{stock + allocated}; }
Money refund(Money stock, Money allocated) {
    return refund(stock.cents, allocated.cents);
}
Money reserve(const Store &store, int units) {
    return store.price(units);
}
}
namespace telemetry {
int refund(int stock, int allocated) { return stock - allocated; }
}
