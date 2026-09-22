# A symbolic SIMD width is a parameter expression in the pin's normal form:
# `SIMD[dt, n + 1]` and `SIMD[dt, 1 + n]` are one type, as are `2 * n` and
# `n + n`, and a struct's own value parameter keys a member's width
# (`SIMD[DType.int64, Self.length]`). Promoted from
# `conformance/probes/param_expr_simd_hooks.mojo`.
def reordered[dt: DType, n: Int](x: SIMD[dt, n + 1]) -> SIMD[dt, 1 + n]:
    return x


def collected[dt: DType, n: Int](x: SIMD[dt, 2 * n]) -> SIMD[dt, n + n]:
    return x


struct Buffer[length: Int](Copyable, Movable):
    var value: Int

    def __init__(out self):
        self.value = 0

    def zeros(self) -> SIMD[DType.int64, Self.length]:
        return SIMD[DType.int64, Self.length](0)


def main():
    print(reordered[DType.int64, 3](SIMD[DType.int64, 4](7)))
    print(collected[DType.int64, 2](SIMD[DType.int64, 4](8)))
    print(Buffer[4]().zeros())
