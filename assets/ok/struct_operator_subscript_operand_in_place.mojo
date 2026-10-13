# A struct operator is its dunder's call, so a place operand the dunder reads
# is read where it lies, as a read argument of a method call is: a list
# element through the `ref`-returning `__getitem__`, a tuple element, or a
# named local, under an infix, reflected, or prefix dunder. Only a chain's
# first middle operand is copied, into the temporary its second link reads.
struct S(ImplicitlyCopyable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)

    def __lt__(self, other: Self) -> Bool:
        print("lt", self.v, other.v)
        return self.v < other.v

    def __add__(self, other: Self) -> Self:
        return S(self.v + other.v)

    def __radd__(self, other: Int) -> Self:
        return S(other + self.v)

    def __neg__(self) -> Self:
        return S(-self.v)

    def __invert__(self) -> Self:
        return S(~self.v)

    def __bool__(self) -> Bool:
        return self.v != 0


def main():
    var xs = List[S]()
    for i in range(1, 5):
        xs.append(S(i))
    print("-- pair")
    print(xs[0] < xs[1])
    print("-- chain")
    print(xs[0] < xs[1] < xs[2])
    print("-- long chain")
    print(xs[0] < xs[1] < xs[2] < xs[3])
    print("-- false first link")
    print(xs[1] < xs[0] < xs[2])
    print("-- if")
    if xs[0] < xs[1]:
        print("yes")
    print("-- sum")
    print((xs[0] + xs[1]).v)
    print("-- neg")
    print((-xs[2]).v)
    print("-- invert")
    print((~xs[1]).v)
    print("-- not")
    print(not xs[0])
    print("-- reflected")
    print((1 + xs[1]).v)
    print("-- named")
    var s = S(5)
    print((-s).v)
    print("-- tuple")
    var t = (S(7), S(8))
    print(t[0] < t[1])
    print("-- end")
