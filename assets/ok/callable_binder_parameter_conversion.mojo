# A call through a callable binder converts its parameter arguments as a
# direct call does: `func[4]` over `func: def[width: SIMDLength](...)` binds
# the callee's `SIMDLength(4)`, an argument over an enclosing binder and a
# keyword argument convert too, and a struct's own `@implicit` constructor
# applies.
def dbl[w: SIMDLength](x: SIMD[DType.int32, w]) -> SIMD[DType.int32, w]:
    return x * 2


def add[w: SIMDLength](
    a: SIMD[DType.int32, w], b: SIMD[DType.int32, w]
) -> SIMD[DType.int32, w]:
    return a + b


def apply[
    func: def[width: SIMDLength](SIMD[DType.int32, width]) thin -> SIMD[
        DType.int32, width
    ]
](x: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return func[4](x)


def apply_n[
    n: Int,
    func: def[width: SIMDLength](SIMD[DType.int32, width]) thin -> SIMD[
        DType.int32, width
    ],
](x: SIMD[DType.int32, n]) -> SIMD[DType.int32, n]:
    return func[n](x)


def apply_kw[
    func: def[width: SIMDLength](SIMD[DType.int32, width]) thin -> SIMD[
        DType.int32, width
    ]
](x: SIMD[DType.int32, 2]) -> SIMD[DType.int32, 2]:
    return func[width=2](x)


def combine[
    func: def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) thin -> SIMD[DType.int32, width]
](x: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return func[4](x, x)


struct Count(ImplicitlyCopyable):
    var n: Int

    @implicit
    def __init__(out self, n: IntLiteral):
        self.n = Int(n)


def times[c: Count](x: Int) -> Int:
    return x * c.n


def apply_count[func: def[c: Count](Int) thin -> Int](x: Int) -> Int:
    return func[3](x)


def main():
    print(apply[dbl](SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(apply_n[8, dbl](SIMD[DType.int32, 8](1, 2, 3, 4, 5, 6, 7, 8)))
    print(apply_kw[dbl](SIMD[DType.int32, 2](5, 6)))
    print(combine[add](SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(apply_count[times](7))
