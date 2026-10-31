# Overloaded methods and initializers whose annotations spell a struct
# `comptime` alias (`Self._mlir_type`, `Self.T`, `Self._T[width]`) are
# declared under the symbol the check spells from their typed signatures,
# the one their calls name.
def add[
    width: SIMDLength
](a: SIMD[DType.int32, width], b: SIMD[DType.int32, width]) -> SIMD[
    DType.int32, width
]:
    return a + b


struct V(Copyable):
    comptime n = 4
    comptime _mlir_type = SIMD[DType.int64, Self.n]
    var v: Self._mlir_type

    def __init__(out self, *, fill: Int):
        self.v = SIMD[DType.int64, Self.n](fill)

    def __init__(out self, *, mlir_value: Self._mlir_type):
        self.v = mlir_value

    def doubled(self) -> Self:
        return Self(mlir_value=self.v + self.v)


struct P[n: Int](Copyable):
    comptime T = SIMD[DType.int32, Self.n]
    var v: Self.T

    def __init__(out self):
        self.v = Self.T(0)

    def __init__(out self, *, raw: Self.T):
        self.v = raw


@fieldwise_init
struct R(Copyable):
    comptime _T = SIMD[DType.int32, _]
    var v: SIMD[DType.int32, 4]

    def red[
        F: ImplicitlyCopyable & RegisterPassable & def[width: SIMDLength](
            Self._T[width], Self._T[width]
        ) -> Self._T[width],
        //,
    ](self, func: F) -> Int32:
        return func(self.v, self.v)[0]

    def red[
        func: def[width: SIMDLength](
            Self._T[width], Self._T[width]
        ) thin -> Self._T[width]
    ](self) -> Int32:
        def helper(a: Int, b: Int) -> Int:
            return a + b

        return func(self.v, self.v)[0] + Int32(helper(0, 0))


def main():
    print(V(fill=3).doubled().v[0])
    print(P[2](raw=SIMD[DType.int32, 2](5)).v[1])
    print(P[2]().v[0])
    var r = R(SIMD[DType.int32, 4](1, 2, 3, 4))
    print(r.red(add))
    print(r.red[add]())
