# The overflow arguments of a variadic constructor are type-checked against the
# element parameter: a `String` cannot bind where `Bag[Int]` expects `Int`, so
# no constructor overload matches. (Previously the single-signature generic
# path silently ignored the overflow arguments and accepted this call.)
# expect: no constructor overload matches

struct Bag[T: Copyable & Movable]:
    var count: Int

    def __init__(out self, var *values: Self.T):
        self.count = len(values)

def main():
    var bad = Bag[Int]("oops", "bad")
    print(bad.count)
