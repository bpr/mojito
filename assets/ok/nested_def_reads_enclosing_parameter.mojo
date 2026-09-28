# A nested `def` reads its enclosing function's value parameters — an `Int`,
# a `Bool`, a `DType`, a callable, and a method's own parameter — without
# naming them in a capture list: a parameter is a compile-time value, not a
# captured local. Beside an explicit `mut` capture it still reads directly.


def scaled[n: Int]() -> Int:
    def inner(x: Int) -> Int:
        return x * n

    return inner(2) + inner(3)


def accumulate[step: Int]() -> Int:
    var total = 0

    def add(x: Int) {mut total}:
        total += x + step

    add(1)
    add(2)
    return total


def pick[flag: Bool]() -> Int:
    def inner() -> Int:
        if flag:
            return 1
        return 0

    return inner()


def lane[dt: DType]() -> Int:
    def inner() -> Int:
        return Int(Scalar[dt](7))

    return inner()


def is_int32[dt: DType]() -> Bool:
    def inner() -> Bool:
        return dt == DType.int32

    return inner()


def double(x: Int) -> Int:
    return x * 2


def through[f: def(Int) thin -> Int]() -> Int:
    def inner() -> Int:
        return f(5)

    return inner()


struct Holder:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def get[k: Int](self) -> Int:
        def inner() -> Int:
            return k * 100

        return inner() + self.base


def main():
    print(scaled[4]())
    print(accumulate[10]())
    print(pick[True](), pick[False]())
    print(lane[DType.int32]())
    print(is_int32[DType.int32](), is_int32[DType.float64]())
    print(through[double]())
    print(Holder(4).get[5]())
