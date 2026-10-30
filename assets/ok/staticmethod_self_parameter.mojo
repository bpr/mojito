# A `@staticmethod` `__init__`'s `out self` is its result, as every
# initializer's is, and another static method's leading unannotated `self`
# is an ordinary parameter of type `Self`.
struct Meters:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    @staticmethod
    def __init__[T: Intable, //](out self, *, doubled: T):
        self = Meters(Int(doubled) * 2)

    @staticmethod
    def total(self, other: Int) -> Int:
        return self.n + other


def main():
    var m = Meters(doubled=UInt8(21))
    print(m.n)
    print(Meters.total(m, 3))
