# Pinned against Mojo 1.2.0.dev2026101705 (2026-10-28).
# `&`, `|`, `^`, `<<`, `>>`, and `~` over a `SIMD`, scalars included, are its
# declared `__and__`, `__or__`, `__xor__`, `__lshift__`, `__rshift__`, and
# `__invert__` (`self ^ -1`), and a literal left operand reaches the right
# one's reflected dunder, as at the pin. An augmented `+=`, `-=`, `*=`, `&=`,
# `|=`, `^=`, `<<=`, or `>>=` calls the target's in-place dunder, whatever
# place it names; and a compile-time `~` folds `__invert__`'s body.


comptime X: Int = 12
comptime W = ~X
comptime V = ~12


struct Bits:
    var n: UInt8

    def __init__(out self):
        self.n = 1

    def widen(mut self):
        self.n <<= 3
        self.n |= 2


struct Cell:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def at[k: Int](mut self) -> ref[self.x] Int:
        return self.x


def mix[dt: DType, w: SIMDLength](v: SIMD[dt, w]) -> SIMD[dt, w]:
    var r = v
    r ^= v << 1
    r += 1
    return ~r & v


def inverted[n: Int]() -> Int:
    comptime m = ~n
    return m


def main():
    var a = 12
    var b = 10
    print(a & b, a | b, a ^ b, ~a, a << 2, a >> 1)
    print(1 & a, 3 | a, 5 ^ a, 1 << b, 4096 >> b)
    var u = UInt(12)
    print(u & 10, u | 3, ~u, u << 1, u >> 2)
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(v & 1, v | 8, v ^ v, ~v, v << 1, v >> 1, 1 << v)
    var m = SIMD[DType.bool, 2](True, False)
    print(m & m, m | m, m ^ m, ~m)
    print(Int8(-128) >> 1, Int8(-128) << 1, UInt8(255) >> 1)
    print(~UInt8(5), ~SIMD[DType.uint16, 2](1, 2), ~Int8(5))
    print(a.__and__(b), a.__rand__(b), a.__invert__(), a.__lshift__(1))
    print(a.__rrshift__(64), a.__xor__(b), a.__ror__(3), a.__rxor__(5))
    var x = 5
    x += 2
    x -= 1
    x *= 3
    x &= 7
    x |= 8
    x ^= 1
    x <<= 2
    x >>= 1
    print(x)
    var w = SIMD[DType.int32, 4](1, 2, 3, 4)
    w += 1
    w *= w
    w <<= 1
    print(w)
    var f = Float64(1.5)
    f += 1
    f *= 2.0
    print(f)
    m &= SIMD[DType.bool, 2](False, True)
    print(m)
    var bits = Bits()
    bits.widen()
    print(bits.n)
    var xs: List[Int] = [1, 2, 3]
    xs[1] += 10
    xs[2] <<= 2
    print(xs[0], xs[1], xs[2])
    var lanes = SIMD[DType.int32, 4](1, 2, 3, 4)
    lanes[1] += 5
    print(lanes)
    var cell = Cell(1)
    cell.at[0]() += 1
    print(cell.x)
    print(mix(SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(W, V, inverted[12]())
    comptime k: UInt8 = 5
    comptime j = ~k
    print(j)
