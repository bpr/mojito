# expect: expected SIMD[DType.int8, n + 2], found SIMD[DType.int8, n + 1]
# A builtin static method call in a parameter expression is its body's
# expression at the instance's parameters: `G[1].plus(n)` is `n + 1`, which
# is not `n + 2`.
struct G[k: Int]:
    @always_inline("builtin")
    @staticmethod
    def plus(n: Int) -> Int:
        return n + Self.k


def g[n: Int](x: SIMD[DType.int8, G[1].plus(n)]) -> SIMD[DType.int8, n + 2]:
    return x


def main():
    print(1)
