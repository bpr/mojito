# `std.simd`'s declared `Bool`, `fill=`, and `Int`-from-`index`
# initializers: a mask splats a `Bool` by `fill=` at any width, a width-one
# mask takes one positionally, and `Int(mlir_value=i)` casts an `index` to
# the `Int`'s storage.
comptime Mask4 = SIMD[DType.bool, 4]

def filled[n: Int](b: Bool) -> SIMD[DType.bool, n]:
    return SIMD[DType.bool, n](fill=b)

def one_lane[n: Int](b: Bool) -> SIMD[DType.bool, n]:
    return SIMD[DType.bool, n](b)

def main():
    print(SIMD[DType.bool, 4](fill=True))
    print(Mask4(fill=False))
    print(filled[8](True))
    print(Scalar[DType.bool](fill=True))
    print(SIMD[DType.bool, 1](True), Scalar[DType.bool](False))
    print(one_lane[1](True))
    var n = 7
    print(Int(mlir_value=n.__mlir_index__()))
    print(SIMD[DType.int, 1](mlir_value=n.__mlir_index__()))
    print(Scalar[DType.int](mlir_value=n.__mlir_index__()))
