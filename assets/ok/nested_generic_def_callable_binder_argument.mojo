# A generic nested `def` passed as a value to a callable-binder parameter
# keeps the instance it is declared in: `red[func]`'s `body[width]` calls the
# enclosing compile-time `func` (upstream's parameter `SIMD.reduce` shape),
# `red_k[k]`'s reads `k`, and a method's reads its struct's `Self.k`, directly
# or from a `comptime for` copy, so each enclosing instance hands `run2` its
# own callee.
def run2[
    F: ImplicitlyCopyable
    & RegisterPassable
    & def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
](v: SIMD[DType.int32, 4], func: F) -> SIMD[DType.int32, 4]:
    return func[4](v, v)


def red[
    func: def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) thin -> SIMD[DType.int32, width]
](v: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    def body[
        width: SIMDLength
    ](lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]) -> SIMD[
        DType.int32, width
    ]:
        return func[width](lhs, rhs)

    return run2(v, body)


def red_k[k: Int](v: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    def body[
        width: SIMDLength
    ](lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]) -> SIMD[
        DType.int32, width
    ]:
        return lhs + rhs + SIMD[DType.int32, width](k)

    return run2(v, body)


@fieldwise_init
struct W[k: Int](ImplicitlyCopyable):
    var v: SIMD[DType.int32, 4]

    def red[
        func: def[width: SIMDLength](
            SIMD[DType.int32, width], SIMD[DType.int32, width]
        ) thin -> SIMD[DType.int32, width]
    ](self) -> SIMD[DType.int32, 4]:
        def body[
            width: SIMDLength
        ](lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]) -> SIMD[
            DType.int32, width
        ]:
            return func[width](lhs, rhs) + SIMD[DType.int32, width](Self.k)

        return run2(self.v, body)

    def unrolled(self) -> SIMD[DType.int32, 4]:
        var acc = self.v
        comptime for i in range(1):

            def body[
                width: SIMDLength
            ](lhs: SIMD[DType.int32, width], rhs: SIMD[DType.int32, width]) -> SIMD[
                DType.int32, width
            ]:
                return lhs + SIMD[DType.int32, width](Self.k + i)

            acc = run2(acc, body)
        return acc


def add[width: SIMDLength](
    x: SIMD[DType.int32, width], y: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return x + y


def mul[width: SIMDLength](
    x: SIMD[DType.int32, width], y: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return x * y


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(red[add](v))
    print(red[mul](v))
    print(red_k[10](v))
    print(W[100](v).red[add]())
    print(W[7](v).red[add]())
    print(W[100](v).unrolled())
    print(W[7](v).unrolled())
