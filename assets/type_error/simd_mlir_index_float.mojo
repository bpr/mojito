# `SIMD.__mlir_index__` is its declared member, whose `comptime assert`
# fails a floating-point instance, as at the pin.
# expect: constraint failed: cannot index using a floating point type
def main():
    print(Int(mlir_value=Float32(1.5).__mlir_index__()))
