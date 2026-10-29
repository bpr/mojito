# A bracketed `__mlir_attr[...]` is its pieces concatenated, as KGEN reads
# them: `` __mlir_attr[`8`, ` : index`] `` is `` __mlir_attr.`8 : index` ``,
# in a `comptime` binding, a struct member, a parameter argument, a type
# argument, and at run time, and an untyped one is a `!kgen.deferred` value.
comptime n: __mlir_type.index = __mlir_attr[`8`, ` : index`]
comptime FAST: __mlir_type.`!kgen.deferred` = __mlir_attr[`#pop.fmf<`, `fast`, `>`]


struct L[k: Int]:
    comptime raw: __mlir_type.index = __mlir_attr[`5`, ` : index`]


struct S[n: __mlir_type.index]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Int(mlir_value=Self.n)


def f[n: __mlir_type.index]() -> Int:
    return Int(mlir_value=n)


def fused(a: Float64, b: Float64, c: Float64) -> Float64:
    comptime none = __mlir_attr[`#pop.fmf<none>`]
    var x = Float64(
        mlir_value=__mlir_op.`pop.fma`[fastmathFlags=FAST](
            a._mlir_value, b._mlir_value, c._mlir_value
        )
    )
    return Float64(
        mlir_value=__mlir_op.`pop.fma`[fastmathFlags=none](
            x._mlir_value, b._mlir_value, c._mlir_value
        )
    )


def main():
    print(Int(mlir_value=n), Int(mlir_value=L[1].raw))
    comptime j: __mlir_type.index = __mlir_attr[`9 : index`]
    print(Int(mlir_value=j), Int(mlir_value=__mlir_attr[`7`, ` : index`]))
    print(f[__mlir_attr[`4`, ` : index`]]())
    var s: S[__mlir_attr[`3`, ` : index`]] = S[__mlir_attr[`3`, ` : index`]]()
    print(s.get())
    print(fused(2.0, 3.0, 1.0))
