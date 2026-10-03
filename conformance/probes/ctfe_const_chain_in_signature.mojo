# Probe: does a module constant chain that applies a function shape a
# signature when both sides spell the same constant?
#
# The pin (2026-10-02) prints `4 8` and `1`: `C` is the symbolic expression
# `mul(f(3), 2)` on both sides, equal by structure. Mojito rejects the
# program: "not a compile-time Int constant: C". docs/roadmap.md 3.121;
# decision D3 of docs/notes/ctfe-request-path.md.
def f(n: Int) -> Int:
    return n + 1

comptime A = 3
comptime B = f(A)
comptime C = B * 2

def g(x: SIMD[DType.float32, C]) -> Int:
    return Int(x[0])

def main():
    print(B, C)
    print(g(SIMD[DType.float32, C](1.0)))
