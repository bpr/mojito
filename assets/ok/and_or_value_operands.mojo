# `and`/`or` over operands that are not both `Bool`, as upstream's
# `emitAndOr`: the left operand is tested through `Bool(x)` and the result is
# the operand the test selects, so operands of a common type yield it (`n or
# 2` is an `Int`, a literal materialized to its partner's type) and any other
# pair yields `Bool`. The right operand runs only when selected. A closed or
# symbolic compile-time `and`/`or` folds the same selection.
@fieldwise_init
struct Flag(Boolable, ImplicitlyCopyable, Writable):
    var v: Int

    def __bool__(self) -> Bool:
        return self.v != 0

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Flag(", self.v, ")")


def side(x: Int) -> Int:
    print("side", x)
    return x


def pick[T: Boolable & ImplicitlyCopyable & Writable](a: T, b: T) -> T:
    return a or b


def with_default[N: Int, M: Int = N or 4]() -> Int:
    return M


def both[N: Int, M: Int = N and 7]() -> Int:
    return M


comptime CLOSED_OR = 0 or 3
comptime CLOSED_AND = 2 and 5
comptime MIXED = 0 or False


def main():
    var n = 0
    var m = 3
    var f = 0.0
    print(n or 2, n and 2, m or side(5), m and side(5))
    print(f or 1.5, f or 3, 2 or n, 0 or n)
    if m and True:
        print("m and True")
    if n and True:
        print("unreachable")
    print(n or True, True and n, f or n)
    var t = n or 4
    print(t + 1)

    var off = Flag(0)
    var on = Flag(7)
    print(off or on, off and on)
    if off or on:
        print("flag or")

    var s = String("")
    print(s or "default", s or String("text"))

    var v = SIMD[DType.int32, 4](1, 0, 3, 4)
    var w = SIMD[DType.int32, 4](0, 0, 3, 5)
    print(v and True, v or False)
    print(v.eq(w) and v.ne(w), v.eq(w) or v.ne(w))

    print(pick(0, 7), pick(String(""), String("s")), pick(Float64(2.5), 1.0))
    var k = 0
    while k < 3 and m:
        k += 1
    print(k, not (m and 0), m and 0 or 9)

    print(CLOSED_OR, CLOSED_AND, MIXED)
    print(with_default[0](), with_default[3](), both[0](), both[2]())
