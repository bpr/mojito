# expect: lacking evidence to prove correctness
# A compile-time call whose callee's `where` clause stays symbolic under the
# call's arguments needs the enclosing declaration to prove it: nothing
# states `m > 0` here.
def h[k: Int](x: Int) -> Int where k > 0:
    return x + k


def f[m: Int, n: Int](x: SIMD[DType.int8, h[m](n)]) -> SIMD[DType.int8, h[m](n)]:
    return x


def main():
    print(f[2, 1](SIMD[DType.int8, 3](1)))
