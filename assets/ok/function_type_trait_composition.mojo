# A function type composes with traits in a binder's bound, after them:
# `F: ImplicitlyCopyable & RegisterPassable & def[...](...) -> ...` binds `F`
# to the argument's type, which a thin function and a closure whose
# captures are trivially register-passable both satisfy.
def dbl[w: SIMDLength](x: SIMD[DType.int32, w]) -> SIMD[DType.int32, w]:
    return x * 2


def apply[
    F: ImplicitlyCopyable & RegisterPassable & def[width: SIMDLength](
        SIMD[DType.int32, width]
    ) -> SIMD[DType.int32, width],
    //,
](func: F, x: SIMD[DType.int32, 4]) -> SIMD[DType.int32, 4]:
    return func[4](x)


def twice[F: TrivialRegisterPassable & def(Int) -> Int](f: F, a: Int) -> Int:
    var g = f
    return g(f(a))


def inc(a: Int) -> Int:
    return a + 1


def main():
    print(apply(dbl, SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(twice(inc, 5))
    var k = 3

    def addk(a: Int) {imm k} -> Int:
        return a + k

    def addv(a: Int) {var k} -> Int:
        return a + k

    print(twice(addk, 5))
    print(twice(addv, 1))
