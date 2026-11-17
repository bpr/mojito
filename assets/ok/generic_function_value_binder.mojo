# A generic function at a `def[...] thin` value binder is its generator's
# symbol with its own binders open, each call through the binder binding
# them: a module function, its default, a nested `def` in a generic body or
# in `main`, a capture-free generic lambda, and a static generic method.


def add[w: SIMDLength](
    x: SIMD[DType.int32, w], y: SIMD[DType.int32, w]
) -> SIMD[DType.int32, w]:
    return x + y


def apply_value[
    func: def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) thin -> SIMD[DType.int32, width] = add
](a: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return func(a, a)


def apply_twice[f: def[n: Int](Int) thin -> Int](x: Int) -> Int:
    return f[2](x) + f[3](x)


def outer[k: Int](a: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    def mul[w: SIMDLength](
        x: SIMD[DType.int32, w], y: SIMD[DType.int32, w]
    ) -> SIMD[DType.int32, w]:
        return x * y + SIMD[DType.int32, w](k)

    return apply_value[mul](a)


def scaled[m: Int](x: Int) -> Int:
    def s[n: Int](v: Int) -> Int:
        return v * n + m

    return apply_twice[s](x)


struct Plain:
    @staticmethod
    def sub[w: SIMDLength](
        x: SIMD[DType.int32, w], y: SIMD[DType.int32, w]
    ) -> SIMD[DType.int32, w]:
        return x - y + 1


def main():
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(apply_value(a))
    print(outer[10](a))
    print(scaled[100](7))

    def plus[w: SIMDLength](
        x: SIMD[DType.int32, w], y: SIMD[DType.int32, w]
    ) -> SIMD[DType.int32, w]:
        return x + y + 1

    print(apply_value[plus](a))
    print(
        apply_value[
            lambda[w: SIMDLength](
                x: SIMD[DType.int32, w], y: SIMD[DType.int32, w]
            ) -> SIMD[DType.int32, w]: x - y
        ](a)
    )
    print(apply_value[Plain.sub](a))
