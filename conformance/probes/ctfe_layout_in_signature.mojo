# Probe: is a compile-time layout query a parameter expression the pin keeps
# symbolic, or a folded constant?
#
# The pin (2026-10-03) keeps it symbolic: `SIMD[.float32, 16]` cannot
# convert to `SIMD[.float32, size_of[Pair]()]`, for the constant `S` and for
# `size_of[T]()` spelled in the signature alike, so both calls are rejected.
# Mojito preserves the same application identity and rejects the literal
# widths at the calls. Matching constructions run in
# assets/ok/comptime_layout_constant.mojo.
from std.sys import size_of

@fieldwise_init
struct Pair(Copyable, Movable):
    var a: Int
    var b: Bool

comptime S = size_of[Pair]()

def g(x: SIMD[DType.float32, S]) -> Int:
    return Int(x[0])

def h[T: AnyType](x: SIMD[DType.float32, size_of[T]()]) -> Int:
    return Int(x[1])

def main():
    print(S)
    print(g(SIMD[DType.float32, 16](2.0)))
    print(g(SIMD[DType.float32, S](3.0)))
    print(h[Pair](SIMD[DType.float32, 16](4.0)))
