# expect: SIMD element
# Several arguments bind upstream's `*elems: Scalar[Self.dtype]`, so a runtime
# `Int` element must be spelled as the lane's scalar (`UInt8(i)`).
def main():
    var i = 300
    print(SIMD[DType.uint8, 4](i, 1, 2, 3))
