# `Bool`'s comparison and bitwise operators are upstream's declared
# dunders in `std.builtin.bool`: spelled as calls, through a type receiver,
# in place, reflected into another operand's `__r*__`, beside a one-lane
# mask, through a `Comparable` bound, chained, and folded at compile time.


struct Flag:
    var on: Bool

    def __init__(out self, on: Bool):
        self.on = on

    def __ror__(self, lhs: Bool) -> Bool:
        return lhs | self.on


def biggest[T: Comparable & ImplicitlyCopyable](a: T, b: T) -> T:
    return b if a < b else a


def main():
    var t = True
    var f = False
    print(t.__eq__(f), t.__ne__(f), t.__invert__())
    print(Bool.__eq__(True, False), Bool.__lt__(False, True))
    print(t == f, t != f, t < f, t <= f, t > f, t >= f)
    print(~t, t & f, t | f, t ^ f)
    print(t.__rand__(f), t.__ror__(f), t.__rxor__(f))
    var b = True
    b.__iand__(False)
    print(b)
    b.__ior__(True)
    b ^= True
    print(b)
    var mask = SIMD[DType.bool, 1](True)
    print(b == mask, mask == b)
    print(f | Flag(True))
    print(biggest(False, True), biggest(True, False))
    print(f < t < t, f < t <= f)
    comptime eq = True == False
    comptime inv = ~True
    comptime lt = False < True
    print(eq, inv, lt)
