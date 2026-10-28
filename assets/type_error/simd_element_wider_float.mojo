# expect: SIMD element
# A `Float64` is no `Scalar[DType.float32]`: several elements are each already
# the lane's scalar, as upstream's `*elems` overload takes them.
def main():
    var f = Float64(2.5)
    print(SIMD[DType.float32, 2](f, f))
