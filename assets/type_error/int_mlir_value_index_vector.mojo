# expect: argument 'mlir_value'
# The initializer from an `index` refines `out self` to `Int`, a one-lane
# vector, so a two-lane vector takes only its own `!kgen.simd` storage.
def main():
    var n = 7
    print(SIMD[DType.int, 2](mlir_value=n.__mlir_index__()))
