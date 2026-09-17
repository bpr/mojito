# `dtype` names the lane dtype of a `SIMD` type or value, as upstream's struct
# parameter: on a scalar alias, a vector type, a value, and a call result
# (which still runs, as it does for `length`), at run time and at compile time.
def make_int32() -> Int32:
    print("make_int32")
    return 1


def make_vector() -> SIMD[DType.int8, 4]:
    print("make_vector")
    return SIMD[DType.int8, 4](1)


def main():
    print(Int32.dtype)
    print(Int.dtype, Float16.dtype, UInt8.dtype, Float64.dtype)
    print(SIMD[DType.uint8, 8].dtype)
    print(Scalar[DType.int8].dtype)
    var s = SIMD[DType.int16, 4](1)
    print(s.dtype)
    var x: Float64 = 1.5
    print(x.dtype)
    var i = 3
    print(i.dtype)
    print(make_int32().dtype)
    var n: Int = Int(make_vector().length)
    print(n)
    var y = s.dtype
    print(y == DType.int16, s.dtype.is_integral())
    comptime d = Int32.dtype
    print(d, d.is_signed())
    comptime e = SIMD[DType.float32, 4].dtype
    var v = SIMD[e, 2](0.5)
    print(v)
    var w = SIMD[Float32.dtype, 2](1.5)
    print(w)
