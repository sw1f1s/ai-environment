class Money:
    value: int


class Base:
    def price(self, amount: Money) -> Money:
        return amount


class Store(Base):
    saved: list[Money]

    def __init__(self, amount: Money):
        self.balance: Money = amount
