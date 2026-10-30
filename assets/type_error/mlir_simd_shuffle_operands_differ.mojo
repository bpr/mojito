# The two operands of `pop.simd.shuffle` are vectors of one type.
# expect: invalid call to 'pop.simd.shuffle': op failed to verify that all of {lhs, rhs} have same type
def main():
    var x = SIMD[DType.int32, 4](1, 2, 3, 4)
    var y = SIMD[DType.int32, 2](1, 2)
    var r = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<1, 0> : !pop.array<2, index>`,
        _type=SIMD[DType.int32, 2]._mlir_type,
    ](x._mlir_value, y._mlir_value)
    print(SIMD[DType.int32, 2](mlir_value=r))
