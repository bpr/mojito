# Initializer overloads rank as free functions do: at equal conversion cost
# the shorter compile-time parameter signature wins, so a `Floatable` and
# `Intable` argument selects the one-parameter overload over the
# two-parameter one whatever their order, as at the pin.


struct F(Floatable, Intable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __int__(self) -> Int:
        return self.v

    def __float__(self) -> Float64:
        return Float64(self.v) + 0.5


struct Defaulted[d: Int]:
    var x: Int

    def __init__[T: Intable, t: Int = 0](out self: Defaulted[t], v: T):
        self.x = 10

    def __init__[T: Floatable, //](out self: Defaulted[1], v: T):
        self.x = 20


struct InferOnly[d: Int]:
    var x: Int

    def __init__[T: Intable, //, t: Int = 0](out self: InferOnly[t], v: T):
        self.x = 10

    def __init__[T: Floatable, //](out self: InferOnly[1], v: T):
        self.x = 20


struct Inferred[d: Int]:
    var x: Int

    def __init__[T: Intable, t: Int](out self: Inferred[t], v: T):
        self.x = 10

    def __init__[T: Floatable, //](out self: Inferred[1], v: T):
        self.x = 20


struct Swapped[d: Int]:
    var x: Int

    def __init__[T: Floatable, t: Int = 0](out self: Swapped[t], v: T):
        self.x = 10

    def __init__[T: Intable, //](out self: Swapped[1], v: T):
        self.x = 20


struct Unused[d: Int]:
    var x: Int

    def __init__[T: Intable, k: Int = 0](out self: Unused[1], v: T):
        self.x = 10

    def __init__[T: Floatable, //](out self: Unused[1], v: T):
        self.x = 20


def pick[T: Intable, k: Int = 0](v: T) -> Int:
    return 10


def pick[T: Floatable, //](v: T) -> Int:
    return 20


def main():
    print(Defaulted[1](F(3)).x, InferOnly[1](F(3)).x)
    print(Inferred[1](F(3)).x, Swapped[1](F(3)).x, Unused[1](F(3)).x)
    print(pick(F(3)))
