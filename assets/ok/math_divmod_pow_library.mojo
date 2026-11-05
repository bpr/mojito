# `divmod` and `pow` are upstream's library functions in `std.math`,
# exported from the prelude: `divmod[T: DivModable]` binds `T` from its
# first operand (a `UInt8` with a literal divisor) and reads its operands in
# place, and `pow` is the two `SIMD` overloads over `__pow__`.
from std.math import pow as mpow, divmod as mdivmod, DivModable


@fieldwise_init
struct Bytes(DivModable, Writable):
    var size: Int

    def __divmod__(self, other: Self) -> Tuple[Self, Self]:
        return (Bytes(self.size // other.size), Bytes(self.size % other.size))


struct Loud(DivModable, Deinitable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)

    def __divmod__(self, denominator: Self) -> Tuple[Self, Self]:
        return (Loud(self.v // denominator.v), Loud(self.v % denominator.v))


def quotient[U: DivModable & Deinitable](x: U, d: U) -> U:
    var y = x
    var parts = divmod(y, d)
    print("after")
    return parts[0]


def lifecycle():
    var a = Loud(7)
    var b = Loud(2)
    var t = divmod(a, b)
    print(t[0].v, t[1].v)
    var q = quotient(a, b)
    print(q.v)
    print(divmod(UInt8(7), 3))


def main():
    lifecycle()
    print(divmod(7.5, 2.0))
    print(divmod(UInt(7), 2))
    print(divmod(Int8(-7), Int8(2)))
    print(pow(2.0, 3))
    print(pow(Float32(2), 2))
    print(pow(Int64(3), Int64(4)))
    print(mpow(3, 2), mdivmod(9, 4))
    var b = divmod(Bytes(10), Bytes(3))
    print(b[0].size, b[1].size)
    var x = 5
    var y = 3
    var q, r = divmod(x, y)
    print(q, r)
    print(round(3), round(Float32(2.5)))
    print(pow(SIMD[DType.float64, 2](4, 9), 0.5))
