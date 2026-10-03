# Probe: does a module constant that applies a function shape a signature?
#
# The pin (2026-10-02) keeps `M` symbolic through the check and rejects the
# call: "value passed to 'x' cannot be converted from 'SIMD[.float32, 8]' to
# 'SIMD[.float32, f(Int(7))]'". Mojito rejects it earlier, in source
# validation: "not a compile-time Int constant: M". Both reject; the pin's
# reason is decision D3 of docs/notes/ctfe-request-path.md.
def f(n: Int) -> Int:
    return n + 1

comptime M = f(7)

def g(x: SIMD[DType.float32, M]) -> Int:
    return Int(x[0])

def main():
    var v = SIMD[DType.float32, 8](3.0)
    print(g(v))
    print(M)
