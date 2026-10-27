# expect: expected SIMD[DType.int8, n + 1], found SIMD[DType.int8, 2 * n]
# A builtin call in a parameter expression is its body's expression:
# `twice(n)` is `n + n`, which is not `n + 1`.
@always_inline("builtin")
def twice(n: Int) -> Int:
    return n + n


def f[n: Int](x: SIMD[DType.int8, twice(n)]) -> SIMD[DType.int8, n + 1]:
    return x


def main():
    print(f[1](SIMD[DType.int8, 2](1)))
