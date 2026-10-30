# A `pop.simd.shuffle` result holds its operands' dtype.
# expect: invalid call to 'pop.simd.shuffle': op expected result dtype to match operand dtypes
def main():
    var x = SIMD[DType.int32, 4](1, 2, 3, 4)
    var r = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<1, 0> : !pop.array<2, index>`,
        _type=SIMD[DType.int64, 2]._mlir_type,
    ](x._mlir_value, x._mlir_value)
    print(SIMD[DType.int64, 2](mlir_value=r))
