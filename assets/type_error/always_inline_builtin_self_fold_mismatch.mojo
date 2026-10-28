# expect: expected SIMD[DType.int8, 2 * n + 1], found SIMD[DType.int8, 2 * n]
# A builtin method's call is its body's expression, so `L(n).doubled()` is
# `2 * n`, which is not `2 * n + 1`.
struct L(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, v: Int):
        self.v = v

    @always_inline("builtin")
    def doubled(self) -> Int:
        return self.v + self.v


def g[n: Int](x: SIMD[DType.int8, L(n).doubled()]) -> SIMD[DType.int8, 2 * n + 1]:
    return x


def main():
    print(1)
