# A call through a callable binder infers the binder's own parameters from
# its arguments, as a call to a generic `def` does: `func(a, b)` over
# `def[width: SIMDLength](...)` binds `width = 4`, an argument typed by an
# enclosing binder solves to that binder, the partial spelling `func[3](a)`
# infers the remaining `width`, and a parenthesized callee infers too.
def add[width: SIMDLength](
    x: SIMD[DType.int32, width], y: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return x + y


def twice[n: Int](x: SIMD[DType.int32, n]) -> SIMD[DType.int32, n]:
    return x + x


def scale[k: Int, width: SIMDLength](
    x: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return x * SIMD[DType.int32, width](k)


def apply[
    F: ImplicitlyCopyable
    & RegisterPassable
    & def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
](
    a: SIMD[DType.int32, 4], b: SIMD[DType.int32, 4], func: F
) -> SIMD[DType.int32, 4]:
    return func(a, b)


def apply_value[
    func: def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) thin -> SIMD[DType.int32, width]
](a: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return func(a, a)


def apply_paren[
    func: def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) thin -> SIMD[DType.int32, width]
](a: SIMD[DType.int32, 4], b: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return (func)(a, b)


def apply_over[
    f: def[n: Int](SIMD[DType.int32, n]) thin -> SIMD[DType.int32, n], m: Int
](a: SIMD[DType.int32, m]) -> SIMD[DType.int32, m]:
    return f(a)


def apply_partial[
    func: def[k: Int, width: SIMDLength](SIMD[DType.int32, width]) thin -> SIMD[
        DType.int32, width
    ]
](a: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return func[3](a)


def main():
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(apply(a, a, add))
    print(apply_value[add](a))
    print(apply_paren[add](a, a))
    print(apply_over[twice, 4](a))
    print(apply_partial[scale](a))
