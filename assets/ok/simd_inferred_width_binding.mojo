# A `SIMD[dt, _]` binding annotation takes its width from the initializer,
# so the binding holds a vector of known lane count that `len` reads.
def main():
    var v: SIMD[DType.int32, _] = SIMD[DType.int32, 4](1, 2, 3, 4)
    print(len(v))
    print(v[2])
    print(v + v)
    var s: SIMD[DType.float32, _] = SIMD[DType.float32, 2](1.5, 2.5)
    print(len(s), s)
    var f: SIMD[DType.float64, _] = SIMD[DType.float64, 1](3.0)
    print(len(f), f)
    print(len(Int32(7)), len(Float64(2.0)))
