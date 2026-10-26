# Compile-time value arguments: a computed call in a bracket, a `UInt`
# parameter inferred from an argument's type, a `UInt`-annotated constant
# read at its declared type, and `DType`-keyed structs named like the
# bundled library's binders.
def fib(n: Int) -> Int:
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


struct W[n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


def g[n: Int]() -> Int:
    return n


struct U[v: UInt]:
    def __init__(out self):
        pass


def unsigned[v: UInt](s: U[v]) -> UInt:
    return v


comptime C: UInt = 3


struct T[dt: DType]:
    def __init__(out self):
        pass

    def show(self):
        print(Self.dt)


struct K[dt: DType]:
    def __init__(out self):
        pass

    def show(self):
        print(Self.dt)


struct H[dt: DType]:
    def __init__(out self):
        pass

    def show(self):
        print(Self.dt)


def main():
    print(g[fib(6)]())
    print(W[fib(6)]().get())
    print(unsigned(U[UInt(7)]()))
    var u = C
    u -= 4
    print(u)
    T[DType.int8]().show()
    K[DType.int16]().show()
    H[DType.float32]().show()
