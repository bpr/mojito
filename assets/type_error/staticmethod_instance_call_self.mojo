# A `@staticmethod`'s leading `self` is an ordinary parameter, not a
# receiver, so an instance call leaves it unbound, as at the pin.
# expect: invalid call to 'total'
struct Meters:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    @staticmethod
    def total(self) -> Int:
        return self.n


def main():
    print(Meters(3).total())
