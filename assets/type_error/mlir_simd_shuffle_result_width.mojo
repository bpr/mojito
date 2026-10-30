# A `pop.simd.shuffle` result has one lane per mask element.
# expect: invalid call to 'pop.simd.shuffle': op expected result to be a vector of 4 elements
def main():
    var x = SIMD[DType.int32, 4](1, 2, 3, 4)
    var r = __mlir_op.`pop.simd.shuffle`[
        mask=__mlir_attr.`#pop.array<1, 0, 2, 3> : !pop.array<4, index>`,
        _type=SIMD[DType.int32, 2]._mlir_type,
    ](x._mlir_value, x._mlir_value)
    print(SIMD[DType.int32, 2](mlir_value=r))
