# expect: are cast incompatible
def main():
    var v = UInt32(4631)
    var r = __mlir_op.`pop.bitcast`[_type=SIMD[DType.uint16, 1]._mlir_type](
        v._mlir_value
    )
    print(SIMD(mlir_value=r))
