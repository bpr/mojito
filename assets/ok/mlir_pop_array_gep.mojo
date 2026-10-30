# `pop.array.gep` addresses one element of a `!pop.array` through a
# `!kgen.pointer` to it, as upstream's `StaticTuple._unsafe_ref` and
# `VariadicList`'s constructor take it; an `index` parameter reads back
# with `Int(mlir_value=)`.


struct Row[n: Int](ImplicitlyCopyable):
    var _mlir_value: __mlir_type[
        `!pop.array<`, Self.n.__mlir_index__(), `, `, Int, `>`
    ]

    def __init__(out self, fill: Int):
        self._mlir_value = __mlir_op.`pop.array.repeat`[
            _type=__mlir_type[
                `!pop.array<`, Self.n.__mlir_index__(), `, `, Int, `>`
            ]
        ](fill)

    def _unsafe_ref(ref self, idx: Int) -> ref[self] Int:
        var ptr = __mlir_op.`pop.array.gep`(
            Pointer(to=self._mlir_value)._get_kgen_pointer(),
            idx.__mlir_index__(),
        )
        return Pointer[origin=origin_of(self)](_mlir_value=ptr)[]


def total[n: Int](ref row: Row[n]) -> Int:
    var sum = 0
    for i in range(n):
        sum += row._unsafe_ref(i)
    return sum


def build() -> Int:
    var r = Row[4](3)
    r._unsafe_ref(2) = 10
    return r._unsafe_ref(0) + r._unsafe_ref(2)


comptime FOLDED = build()


def count[size: __mlir_type.index]() -> Int:
    return Int(mlir_value=size)


def main():
    var r = Row[3](7)
    r._unsafe_ref(1) = 9
    print(r._unsafe_ref(0), r._unsafe_ref(1), r._unsafe_ref(2), total(r))
    var rows = List[Row[2]]()
    rows.append(Row[2](1))
    rows.append(Row[2](2))
    rows[1]._unsafe_ref(0) = 40
    print(rows[1]._unsafe_ref(0), total(rows[1]))
    var first = Pointer[_, ImmUntrackedOrigin](
        _mlir_value=__mlir_op.`pop.array.gep`(
            Pointer(to=r._mlir_value)._get_kgen_pointer(),
            Int(0).__mlir_index__(),
        )
    )
    print(first[], r._unsafe_ref(2))
    print(FOLDED, count[Int(4).__mlir_index__()]())
