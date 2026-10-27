# A `!kgen.simd` length is an `index`: a `SIMDLength`'s `_mlir_value`. An
# `Int`'s `_mlir_value` is a scalar `!kgen.simd`, so the pin rejects it.
# expect: expected 'index' (a SIMDLength's '_mlir_value')
struct Lanes[dtype: DType, length: Int]:
    var _mlir_value: __mlir_type[
        `!kgen.simd<`,
        Self.length._mlir_value,
        `, `,
        Self.dtype._mlir_value,
        `>`,
    ]


def main():
    pass
