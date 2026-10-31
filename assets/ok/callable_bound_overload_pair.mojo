# An overload whose parameter is bounded by a generic callable contract
# names one symbol at its declaration and at its call: the bound spells as
# its checked type, `def[width: SIMDLength](SIMD[DType.int32, width], ...)`,
# beside a same-named overload taking the callable as a compile-time
# parameter, for a method pair, a pair whose parameter overload holds a
# nested `def`, and a free-function pair.
def add[width: SIMDLength](
    a: SIMD[DType.int32, width], b: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return a + b


@fieldwise_init
struct V(Copyable):
    var v: SIMD[DType.int32, 4]

    def red[
        F: ImplicitlyCopyable
        & RegisterPassable
        & def[width: SIMDLength](
            SIMD[DType.int32, width], SIMD[DType.int32, width]
        ) -> SIMD[DType.int32, width],
        //,
    ](self, func: F) -> Int32:
        return func(self.v, self.v)[0]

    def red[
        func: def[width: SIMDLength](
            SIMD[DType.int32, width], SIMD[DType.int32, width]
        ) thin -> SIMD[DType.int32, width]
    ](self) -> Int32:
        return func(self.v, self.v)[1]


@fieldwise_init
struct W(Copyable):
    var v: SIMD[DType.int32, 4]

    def red[
        F: ImplicitlyCopyable
        & RegisterPassable
        & def[width: SIMDLength](
            SIMD[DType.int32, width], SIMD[DType.int32, width]
        ) -> SIMD[DType.int32, width],
        //,
    ](self, func: F) -> Int32:
        return func(self.v, self.v)[2]

    def red[
        func: def[width: SIMDLength](
            SIMD[DType.int32, width], SIMD[DType.int32, width]
        ) thin -> SIMD[DType.int32, width]
    ](self) -> Int32:
        def helper(a: Int, b: Int) -> Int:
            return a + b

        return func(self.v, self.v)[helper(1, 2)]


def red[
    F: ImplicitlyCopyable
    & RegisterPassable
    & def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
](v: SIMD[DType.int32, 4], func: F) -> Int32:
    return func(v, v)[0]


def red[
    func: def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) thin -> SIMD[DType.int32, width]
](v: SIMD[DType.int32, 4]) -> Int32:
    return func(v, v)[3]


def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(V(v).red(add))
    print(V(v).red[add]())
    print(W(v).red(add))
    print(W(v).red[add]())
    print(red(v, add))
    print(red[add](v))
