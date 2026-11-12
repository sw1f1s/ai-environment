from model import Money as Cash, Store
import decoy


def run(store: Store, amount: Cash) -> Cash:
    return store.price(amount)


def unknown(handler):
    return handler()
