# expect: expected SIMD[DType.int8, 2 * n + 1], found SIMD[DType.int8, 2 * n]
# A builtin `__init__` call in a parameter expression is the struct value its
# body stores, so `L(n).v * 2` is `2 * n`, which is not `2 * n + 1`.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v


def g[n: Int](x: SIMD[DType.int8, L(n).v * 2]) -> SIMD[DType.int8, 2 * n + 1]:
    return x


def main():
    print(1)
