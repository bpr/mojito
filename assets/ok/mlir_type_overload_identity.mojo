# An overloaded constructor or method whose parameter is spelled as an MLIR
# type (`__mlir_type.index`, `__mlir_type[`!kgen.simd<`, ...]`) or as a
# vector over the struct's own parameters (`SIMD[Self.dt, Self.n]`) is
# named the same way at its declaration and at its call.


struct Index(ImplicitlyCopyable, TrivialRegisterPassable):
    var _mlir_value: __mlir_type.index

    def __init__(out self, *, mlir_value: __mlir_type.index):
        self._mlir_value = mlir_value

    def __init__(out self, value: Int, /):
        self._mlir_value = __mlir_op.`pop.cast_to_builtin`[
            _type=__mlir_type.index
        ](value._mlir_value)

    def get(self) -> Int:
        return Int(mlir_value=self._mlir_value)


struct Lanes[dt: DType, n: SIMDLength](ImplicitlyCopyable):
    var x: Int

    def __init__(
        out self,
        *,
        storage: __mlir_type[
            `!kgen.simd<`, Self.n._mlir_value, `, `, Self.dt._mlir_value, `>`
        ],
    ):
        var v = SIMD[Self.dt, Self.n](mlir_value=storage)
        self.x = Int(v[1])

    def __init__(out self, v: SIMD[Self.dt, Self.n]):
        self.x = Int(v[2])

    def __init__(out self, value: Int, flag: Bool):
        self.x = value

    def plus(self, v: SIMD[Self.dt, Self.n]) -> Int:
        return Int(v[0]) + self.x

    def plus(self, v: Int) -> Int:
        return v + self.x


def main():
    var a = Index(mlir_value=Int(3).__mlir_index__())
    print(a.get(), Index(5).get())
    var s = SIMD[DType.int32, 4](1, 2, 3, 4)
    var b = Lanes[DType.int32, 4](storage=s._mlir_value)
    var c = Lanes[DType.int32, 4](s)
    print(b.x, c.x, Lanes[DType.int32, 4](7, True).x)
    print(b.plus(s), b.plus(10))
