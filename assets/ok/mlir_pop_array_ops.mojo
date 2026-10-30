# `!pop.array<n, T>` is spelled with a parameter size, built by
# `pop.array.repeat`, read by `pop.array.get`, and rebuilt by
# `pop.array.replace`, as upstream's `StaticTuple` stores its elements;
# a `#pop.array` attribute is a compile-time value of its own type.

comptime REVERSE = __mlir_attr.`#pop.array<3, 2, 1, 0> : !pop.array<4, index>`


struct Row[n: Int](ImplicitlyCopyable):
    var _mlir_value: __mlir_type[
        `!pop.array<`, Self.n.__mlir_index__(), `, `, Int, `>`
    ]

    def __init__(
        out self,
        *,
        mlir_value: __mlir_type[
            `!pop.array<`, Self.n.__mlir_index__(), `, `, Int, `>`
        ],
    ):
        self._mlir_value = mlir_value

    def get[i: Int](self) -> Int:
        return __mlir_op.`pop.array.get`[_type=Int, index=i.__mlir_index__()](
            self._mlir_value
        )

    def replaced[i: Int](self, value: Int) -> Self:
        return Self(
            mlir_value=__mlir_op.`pop.array.replace`[
                _type=__mlir_type[
                    `!pop.array<`, Self.n.__mlir_index__(), `, `, Int, `>`
                ],
                index=i.__mlir_index__(),
            ](value, self._mlir_value)
        )


def filled[n: Int](fill: Int) -> Row[n]:
    return Row[n](
        mlir_value=__mlir_op.`pop.array.repeat`[
            _type=__mlir_type[`!pop.array<`, n.__mlir_index__(), `, `, Int, `>`]
        ](fill)
    )


comptime FIVES = filled[3](5)
comptime FOLDED = FIVES.replaced[2](8)


def main():
    var a = filled[3](7)
    var b = a.replaced[1](9)
    print(a.get[1](), b.get[0](), b.get[1](), b.get[2]())
    print(FOLDED.get[1](), FOLDED.get[2]())
    var c = __mlir_op.`pop.array.repeat`[
        _type=__mlir_type[`!pop.array<2, `, Float64, `>`]
    ](Float64(1.5))
    print(__mlir_op.`pop.array.get`[_type=Float64, index=(1).__mlir_index__()](c))
    var d = __mlir_op.`pop.array.repeat`[_type=__mlir_type.`!pop.array<2, index>`](
        Int(4).__mlir_index__()
    )
    print(
        Int(
            mlir_value=__mlir_op.`pop.array.get`[
                _type=__mlir_type.index, index=(0).__mlir_index__()
            ](d)
        )
    )
    print(
        Int(
            mlir_value=__mlir_op.`pop.array.get`[
                _type=__mlir_type.index, index=(1).__mlir_index__()
            ](REVERSE)
        )
    )
    var v = SIMD[DType.int64, 4](1, 2, 3, 4)
    var r = __mlir_op.`pop.simd.shuffle`[
        mask=REVERSE, _type=SIMD[DType.int64, 4]._mlir_type
    ](v._mlir_value, v._mlir_value)
    print(SIMD[DType.int64, 4](mlir_value=r))
