# A module constant that applies a function keeps its identity through the
# check (decision D3, `docs/notes/ctfe-request-path.md`): `C` is the
# application `f(3) * 2` on both sides of `g`, one canonical node, so the call
# matches, and the elaborator's folded `8` is what the body runs on. The pin
# prints the same (`conformance/probes/ctfe_const_chain_in_signature.mojo`).
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
