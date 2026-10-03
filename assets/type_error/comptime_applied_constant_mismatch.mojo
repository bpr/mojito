# expect: expected SIMD[DType.float32, f(7)], found SIMD[DType.float32, 8]
# A module constant that applies a function is its application in a type,
# never the folded value: the pin rejects `SIMD[.float32, 8]` against
# `SIMD[.float32, f(Int(7))]` (`conformance/probes/ctfe_const_in_signature.mojo`),
# and so does Mojito.
def f(n: Int) -> Int:
    return n + 1

comptime M = f(7)

def g(x: SIMD[DType.float32, M]) -> Int:
    return Int(x[0])

def main():
    var v = SIMD[DType.float32, 8](3.0)
    print(g(v))
