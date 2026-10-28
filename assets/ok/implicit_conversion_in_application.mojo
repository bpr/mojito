# A compile-time call's argument of another type converts to its struct
# parameter through the struct's `@implicit` constructor, as the run-time
# call's argument does: `L(4) + 1` applies `__add__` to `L(4)` and `L(1)`,
# in a parameter argument and over a binder alike, as at the pin.


struct L(ImplicitlyCopyable, TrivialRegisterPassable):
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

    @always_inline("builtin")
    def __mul__(self, rhs: Self) -> Self:
        return Self(
            mlir_value=__mlir_op.`index.mul`(self._mlir_value, rhs._mlir_value)
        )

    @always_inline("builtin")
    def twice(self) -> Self:
        return self * 2


struct Tag[w: L]:
    def __init__(out self):
        pass


def show[w: L]():
    print(Int(mlir_value=w._mlir_value))


def over[w: L]():
    show[w + 1]()
    show[w * 2]()
    show[w.twice() + L(1)]()


def from_int[n: Int]():
    show[L(n) + n]()


def main():
    show[L(4) + 1]()
    over[4]()
    from_int[3]()
    var t: Tag[L(1) + 2] = Tag[L(3)]()
    _ = t^
    print("ok")
