# Upstream's casts between the MLIR builtin `index` and an `Int`'s
# `!kgen.scalar<index>` storage: `pop.cast_from_builtin`,
# `pop.cast_to_builtin`, `Int.__mlir_index__()`, and the `Int(mlir_value=)`
# overload that takes an `index`, at run time and in compile-time bodies.


def take(x: __mlir_type.index) -> Int:
    return Int(mlir_value=x)


def twice(x: Int) -> Int:
    var i = x.__mlir_index__()
    return Int(mlir_value=__mlir_op.`index.add`(i, i))


struct Len(TrivialRegisterPassable):
    var _mlir_value: __mlir_type.index

    @always_inline("builtin")
    def __init__(out self, value: Int):
        self._mlir_value = __mlir_op.`pop.cast_to_builtin`[
            _type=__mlir_type.index
        ](value._mlir_value)

    @always_inline("builtin")
    def get(self) -> Int:
        return Int(mlir_value=self._mlir_value)


struct Holder[n: Len]:
    @staticmethod
    def show():
        print(Self.n.get())


@fieldwise_init
struct Offset(Indexer):
    var value: Int

    def __mlir_index__(self) -> __mlir_type.index:
        return self.value.__mlir_index__()


def main():
    var idx: __mlir_type.index = Int(42).__mlir_index__()
    var storage = __mlir_op.`pop.cast_from_builtin`[
        _type=__mlir_type.`!kgen.scalar<index>`
    ](idx)
    var back = __mlir_op.`pop.cast_to_builtin`[_type=__mlir_type.index](
        storage
    )
    print(Int(mlir_value=back), Int(mlir_value=storage))

    print(take(Int(9).__mlir_index__()))
    var u = UInt(4)
    print(Int(mlir_value=u.__mlir_index__()))
    var s: __mlir_type.`!kgen.scalar<index>` = __mlir_op.`pop.cast_from_builtin`[
        _type=__mlir_type.`!kgen.scalar<index>`
    ](Int(3).__mlir_index__())
    print(Int(mlir_value=__mlir_op.`pop.add`(s, s)))

    var n = Len(5)
    print(n.get(), twice(n.get()))

    var values = [3, 7, 11]
    print(values[Offset(2)])

    comptime k = Int(mlir_value=Int(3).__mlir_index__()) * 2
    comptime t = twice(5)
    comptime l = Len(7).get() * 2
    print(k, t, l)
    Holder[Len(6)].show()
