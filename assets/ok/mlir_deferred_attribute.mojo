# An untyped `__mlir_attr` is a compile-time `!kgen.deferred` value: a
# `comptime` binding holds one, a function returns one, and a `pop` op's
# `fastmathFlags` takes either, closed or over a parameter.
comptime FAST: __mlir_type.`!kgen.deferred` = __mlir_attr.`#pop.fmf<fast>`


def flags(contract: Bool) -> __mlir_type.`!kgen.deferred`:
    if contract:
        return __mlir_attr.`#pop.fmf<contract>`
    return __mlir_attr.`#pop.fmf<none>`


def fused[contract: Bool](a: Float64, b: Float64, c: Float64) -> Float64:
    comptime none = __mlir_attr.`#pop.fmf<none>`
    var x = Float64(
        mlir_value=__mlir_op.`pop.fma`[fastmathFlags=flags(contract)](
            a._mlir_value, b._mlir_value, c._mlir_value
        )
    )
    var y = Float64(
        mlir_value=__mlir_op.`pop.fma`[fastmathFlags=FAST](
            x._mlir_value, b._mlir_value, c._mlir_value
        )
    )
    return Float64(
        mlir_value=__mlir_op.`pop.fma`[fastmathFlags=none](
            y._mlir_value, b._mlir_value, c._mlir_value
        )
    )


def main():
    print(fused[True](2.0, 3.0, 1.0), fused[False](0.1, 10.0, -1.0))
