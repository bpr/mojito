# A `type_of` operand reads only its operand's type, so a `comptime assert`
# over it in a generic function holds no run-time read.
from std.sys import bit_width_of


def h[d: DType, w: SIMDLength](v: SIMD[d, w]) -> Int:
    comptime assert bit_width_of[type_of(v)]() > 0, "a sized vector"
    return bit_width_of[type_of(v)]()


def main():
    print(h(Float32(1.5)))
    print(h(SIMD[DType.int16, 4](1)))
