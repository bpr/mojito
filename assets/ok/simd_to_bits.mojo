# `to_bits` reinterprets each lane as its unsigned bit pattern, zero-extended
# into the requested unsigned lane width.
def main():
    var i8 = SIMD[DType.int8, 4](-1, -128, 127, 0)
    print(i8.to_bits[DType.uint8](), i8.to_bits[DType.uint16](), i8.to_bits[DType.uint32](), i8.to_bits[DType.uint64]())
    var u16 = SIMD[DType.uint16, 4](65535, 256, 1, 0)
    print(u16.to_bits[DType.uint16](), u16.to_bits[DType.uint32](), u16.to_bits[DType.uint64]())
    var i32 = SIMD[DType.int32, 2](-2, 2147483647)
    print(i32.to_bits[DType.uint32](), i32.to_bits[DType.uint64]())
    var f32 = SIMD[DType.float32, 4](1.0, -2.0, 0.0, 0.1)
    print(f32.to_bits[DType.uint32](), f32.to_bits[DType.uint64]())
    var f64 = SIMD[DType.float64, 4](1.0, -2.0, 0.1, -0.0)
    print(f64.to_bits[DType.uint64]())
    var zero = SIMD[DType.float64, 2](0.0)
    var special = SIMD[DType.float64, 2](1.0, -1.0) / zero
    print(special.to_bits[DType.uint64](), (zero / zero).to_bits[DType.uint64]() > 0)
    var b = SIMD[DType.bool, 4](True, False, True, True)
    print(b.to_bits[DType.uint8](), b.to_bits[DType.uint64]())
    var i64 = SIMD[DType.int64, 2](-1, -9223372036854775807 - 1)
    print(i64.to_bits[DType.uint64]())
    var n = SIMD[DType.int, 2](-1, 5)
    print(n.to_bits[DType.uint64]())
