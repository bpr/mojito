# expect: invalid call to 'h': violated constraint
# A compile-time call binds its callee's `where` clause as a runtime call
# does: `h[0](n)` in a signature violates `k > 0`.
def h[k: Int](x: Int) -> Int where k > 0:
    return x + k


def f[n: Int](x: SIMD[DType.int8, h[0](n)]) -> SIMD[DType.int8, h[0](n)]:
    return x


def main():
    print(f[1](SIMD[DType.int8, 1](1)))
