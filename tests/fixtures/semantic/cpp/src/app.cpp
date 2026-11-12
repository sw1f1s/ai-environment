// Межфайловая проверка — 雪 😀
#include "warehouse.hpp"
namespace warehouse {
Money run() {
    Money cash{5};
    Money reserved = refund(8, 2);
    Money combined = refund(cash, reserved);
    DiscountStore store;
    return identity<Money>(reserve(store, combined.cents));
}
}
