# A `pop.simd.shuffle` mask indexes its two operands' lanes, so an element at
# or above twice their width is the pin's verifier error.
# expect: invalid call to 'pop.simd.shuffle': op mask element 9 is out of bounds
def main():
    var a = SIMD[DType.int32, 4](1, 2, 3, 4)
    var r = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<9, 2, 4, 0> : !pop.array<4, index>`,
        _type=SIMD[DType.int32, 4]._mlir_type,
    ](a._mlir_value, a._mlir_value)
    print(SIMD[DType.int32, 4](mlir_value=r))
