# An `@always_inline("builtin")` constructor from `IntLiteral` storing
# `value.__mlir_index__()` folds, so a literal-built value equals the same
# value built from an `Int`, from its `index` storage, or by arithmetic, as
# at the pin.


struct Len(ImplicitlyCopyable, TrivialRegisterPassable):
    var _mlir_value: __mlir_type.index

    @always_inline("builtin")
    def __init__(out self, *, mlir_value: __mlir_type.index):
        self._mlir_value = mlir_value

    @implicit
    @always_inline("builtin")
    def __init__(out self, value: IntLiteral, /):
        self._mlir_value = value.__mlir_index__()

    @implicit
    @always_inline("builtin")
    def __init__(out self, value: Int, /):
        self._mlir_value = __mlir_op.`pop.cast_to_builtin`[
            _type=__mlir_type.index
        ](value._mlir_value)

    @always_inline("builtin")
    def __add__(self, rhs: Self) -> Self:
        return Self(
            mlir_value=__mlir_op.`index.add`(self._mlir_value, rhs._mlir_value)
        )


struct Tag[w: Len]:
    def __init__(out self):
        pass

    @staticmethod
    def show():
        print(Int(mlir_value=Self.w._mlir_value))


def shifted[n: Int]():
    var t: Tag[Len(n) + Len(1)] = Tag[Len(n + 1)]()
    t.show()


def main():
    var a: Tag[Len(3)] = Tag[Len(mlir_value=Int(3).__mlir_index__())]()
    a.show()
    var b: Tag[Len(1) + Len(2)] = Tag[Len(1) + Len(2) + Len(0)]()
    b.show()
    var c: Tag[Len(1) + Len(2)] = Tag[Len(3)]()
    c.show()
    shifted[4]()
