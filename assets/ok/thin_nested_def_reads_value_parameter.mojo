# A nested `def` that reads only its enclosing function's value parameters is
# `thin`: a parameter is a compile-time value, so the function captures
# nothing and passes where a `def() thin` is expected — through an
# intermediate nested `def`, over a `DType` or callable parameter, and in a
# method's own parameter list alike.


def apply(f: def() thin -> Int) -> Int:
    return f()


def scaled[n: Int]() -> Int:
    def inner() -> Int:
        return n * 2

    return apply(inner)


def nested[n: Int, m: Int]() -> Int:
    def middle() -> Int:
        def inner() -> Int:
            return n + m

        return apply(inner) * 10

    return apply(middle)


def width[dt: DType]() -> Int:
    def inner() -> Int:
        if dt == DType.int32:
            return 32
        return 64

    return apply(inner)


def double(x: Int) -> Int:
    return x * 2


def through[f: def(Int) thin -> Int]() -> Int:
    def inner() -> Int:
        return f(5)

    return apply(inner)


struct Holder:
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def get[k: Int](self) -> Int:
        def inner() -> Int:
            return k * 100

        return apply(inner) + self.base


def main():
    print(scaled[3](), scaled[10]())
    print(nested[1, 2]())
    print(width[DType.int32](), width[DType.float64]())
    print(through[double]())
    print(Holder(4).get[5]())
