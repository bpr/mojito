# A comparison through an `Equatable`/`Comparable` bound is its
# requirement's call: a copied operand dies when the comparison returns,
# before the result's consumer runs.
struct S(Comparable, Copyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v

    def __lt__(self, other: Self) -> Bool:
        return self.v < other.v

    def __eq__(self, other: Self) -> Bool:
        return self.v == other.v

    def __deinit__(deinit self):
        print("del", self.v)


def less[T: Comparable & Copyable & Deinitable](a: T):
    var x = a.copy()
    print(x < a)
    print("mid")
    var y = a.copy()
    print(a < y)


def equal[T: Comparable & Copyable & Deinitable](a: T):
    var x = a.copy()
    print(x == a)
    print("mid")
    var y = a.copy()
    print(y != a)


def main():
    less(S(9))
    print("--")
    equal(S(9))
    print("--")
    less(3)
    equal(3)
    print("--")
    less(String("a"))
    equal(String("a"))
