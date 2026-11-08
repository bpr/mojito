# A `pop` op's `fastmathFlags` attribute, as at the pin: each instance
# closes a computed flag (`fmul contract` in `scaled[True]`, a plain `fmul`
# in `scaled[False]`), a `|`-joined set is one attribute, and integer lanes
# ignore the flag. Exact arithmetic implements every flag.
from std.simd import FastMathFlag


def flags(contract: Bool) -> __mlir_type.`!kgen.deferred`:
    if contract:
        return __mlir_attr.`#pop.fmf<contract>`
    return __mlir_attr.`#pop.fmf<none>`


@no_inline
def scaled[contract: Bool](a: Float32) -> Float32:
    return Float32(
        mlir_value=__mlir_op.`pop.mul`[fastmathFlags=flags(contract)](
            a._mlir_value, a._mlir_value
        )
    )


@no_inline
def joined(a: SIMD[DType.float64, 2], b: SIMD[DType.float64, 2]) -> SIMD[DType.float64, 2]:
    return SIMD[DType.float64, 2](
        mlir_value=__mlir_op.`pop.sub`[fastmathFlags=__mlir_attr.`#pop.fmf<nnan | ninf>`](
            a._mlir_value, b._mlir_value
        )
    )


@no_inline
def wraps(a: Int32, b: Int32) -> Int32:
    return Int32(
        mlir_value=__mlir_op.`pop.add`[fastmathFlags=__mlir_attr.`#pop.fmf<fast>`](
            a._mlir_value, b._mlir_value
        )
    )


@no_inline
def widened(a: Float32) -> Float64:
    return Float64(
        mlir_value=__mlir_op.`pop.cast`[
            _type=Float64._mlir_type,
            fastmathFlags=__mlir_attr.`#pop.fmf<reassoc>`,
        ](a._mlir_value)
    )


def main():
    print(scaled[True](1.5), scaled[False](2.0))
    print(joined(SIMD[DType.float64, 2](5.0, 1.25), SIMD[DType.float64, 2](2.0, 0.5)))
    print(wraps(Int32(2147483647), Int32(1)))
    print(widened(Float32(0.5)))
    print(Float32(2.0).fma[FastMathFlag.REASSOC](3.0, 1.0))
