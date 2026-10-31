# expect: cannot infer type parameter 'dtype'
# `SIMD`'s `mlir_value=` initializer infers `dtype` and `length` only from
# storage a binder spells; a concrete vector's storage binds neither.
def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(SIMD(mlir_value=v._mlir_value))
