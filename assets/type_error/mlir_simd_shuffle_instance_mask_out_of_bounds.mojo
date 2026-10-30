# A width-keyed template's `pop.simd.shuffle` mask is checked against each
# instance's width: `f[4]` is in bounds, and `f[2]` fails its instance.
# expect: function instantiation of `f` failed: 'pop.simd.shuffle' op mask element 5 is out of bounds
def f[n: Int](x: SIMD[DType.int32, n]) -> SIMD[DType.int32, 2]:
    return SIMD[DType.int32, 2](
        mlir_value=__mlir_op.`pop.simd.shuffle`[
            mask=__mlir_attr.`#pop.array<5, 0> : !pop.array<2, index>`,
            _type=SIMD[DType.int32, 2]._mlir_type,
        ](x._mlir_value, x._mlir_value)
    )


def main():
    print(f[4](SIMD[DType.int32, 4](1, 2, 3, 4)))
    print(f[2](SIMD[DType.int32, 2](1, 2)))
