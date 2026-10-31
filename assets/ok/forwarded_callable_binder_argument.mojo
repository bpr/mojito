# A value of a callable-binder-bounded type parameter keeps its callee when
# it is passed on to another generic call: `outer` forwards `func` to
# `inner`, which calls `func[n]`; a recursive free function and a recursive
# method (upstream's run-time `SIMD.reduce` shape) forward it to themselves
# at half the width; and a plain `def(Int) -> Int` bound forwards the same
# way.
def add[width: SIMDLength](
    x: SIMD[DType.int32, width], y: SIMD[DType.int32, width]
) -> SIMD[DType.int32, width]:
    return x + y


def add1(x: Int) -> Int:
    return x + 1


def inner[
    F: ImplicitlyCopyable
    & RegisterPassable
    & def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
    n: SIMDLength,
](x: SIMD[DType.int32, n], func: F) -> SIMD[DType.int32, n]:
    return func[n](x, x)


def outer[
    F: ImplicitlyCopyable
    & RegisterPassable
    & def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
    n: SIMDLength,
](x: SIMD[DType.int32, n], func: F) -> SIMD[DType.int32, n]:
    return inner(x, func)


def tree[
    F: ImplicitlyCopyable
    & RegisterPassable
    & def[width: SIMDLength](
        SIMD[DType.int32, width], SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
    n: SIMDLength,
](x: SIMD[DType.int32, n], func: F) -> Int32:
    comptime if n == 1:
        return x[0]
    else:
        var lhs = x.slice[n // 2]()
        var rhs = x.slice[n // 2, offset = n // 2]()
        return tree(func[n // 2](lhs, rhs), func)


@fieldwise_init
struct V[n: SIMDLength](ImplicitlyCopyable):
    var v: SIMD[DType.int32, Self.n]

    def red[
        F: ImplicitlyCopyable
        & RegisterPassable
        & def[width: SIMDLength](
            SIMD[DType.int32, width], SIMD[DType.int32, width]
        ) -> SIMD[DType.int32, width],
        //,
    ](self, func: F) -> Int32:
        comptime if Self.n == 1:
            return self.v[0]
        else:
            var lhs = self.v.slice[Self.n // 2]()
            var rhs = self.v.slice[Self.n // 2, offset = Self.n // 2]()
            return V[Self.n // 2](func(lhs, rhs)).red(func)


def inner_int[
    F: ImplicitlyCopyable & RegisterPassable & def(Int) -> Int, //
](x: Int, func: F) -> Int:
    return func(x)


def outer_int[
    F: ImplicitlyCopyable & RegisterPassable & def(Int) -> Int, //
](x: Int, func: F) -> Int:
    return inner_int(x, func)


def main():
    print(outer(SIMD[DType.int32, 4](1, 2, 3, 4), add))
    print(tree(SIMD[DType.int32, 8](1, 2, 3, 4, 5, 6, 7, 8), add))
    print(V[8](SIMD[DType.int32, 8](1, 2, 3, 4, 5, 6, 7, 8)).red(add))
    print(outer_int(3, add1))
