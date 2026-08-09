# Scalar[DType.x](arg) constructs a width-1 SIMD scalar: non-canonical
# dtypes wrap at the element width; DType.int and DType.float64 canonicalize
# to the native Int and Float64.
def main():
    print(Scalar[DType.uint8](300))
    print(Scalar[DType.float64](3))
    var i: Int = Scalar[DType.int](41 + 1)
    print(i)
    var v: SIMD[DType.int, 4] = SIMD[DType.int, _](1, 2, 3, 4)
    print(v[2])
