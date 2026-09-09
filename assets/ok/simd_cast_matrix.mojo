# Elementwise casts: integer lanes re-wrap at the target width in every
# direction, integer-to-float rounds through f64 (Float32 rounds again),
# float widening is exact, narrowing rounds, and float-to-int truncates
# toward zero, saturates at the 128-bit intermediate, and then wraps.
def main():
    var i32 = SIMD[DType.int32, 4](-1, 300, -70000, 2147483647)
    print(i32.cast[DType.int8](), i32.cast[DType.uint8](), i32.cast[DType.int16](), i32.cast[DType.uint16]())
    print(i32.cast[DType.int64](), i32.cast[DType.uint64](), i32.cast[DType.uint32](), i32.cast[DType.int]())
    var u8 = SIMD[DType.uint8, 4](255, 128, 1, 0)
    print(u8.cast[DType.int8](), u8.cast[DType.int16](), u8.cast[DType.uint64](), u8.cast[DType.float32](), u8.cast[DType.float64]())
    var i64 = SIMD[DType.int64, 4](16777217, -16777217, 9007199254740993, -9223372036854775807 - 1)
    print(i64.cast[DType.float32](), i64.cast[DType.float64](), i64.cast[DType.int32](), i64.cast[DType.uint16]())
    var u64 = SIMD[DType.uint64, 2](18446744073709551615, 9223372036854775808)
    print(u64.cast[DType.float64](), u64.cast[DType.float32](), u64.cast[DType.int64](), u64.cast[DType.int8]())
    var f32 = SIMD[DType.float32, 4](0.1, 2.9, -3.9, 16777216.0)
    print(f32.cast[DType.float64](), f32.cast[DType.int32](), f32.cast[DType.uint8](), f32.cast[DType.int64]())
    var f64 = SIMD[DType.float64, 4](0.1, 1e30, -1e30, 3.999999999)
    print(f64.cast[DType.float32](), f64.cast[DType.int32](), f64.cast[DType.int8](), f64.cast[DType.uint64](), f64.cast[DType.int]())
    var zero = SIMD[DType.float64, 4](0.0)
    var special = SIMD[DType.float64, 4](1.0, -1.0, 0.0, 9223372036854775808.0) / SIMD[DType.float64, 4](0.0, 0.0, 0.0, 1.0)
    print(special, special.cast[DType.int8](), special.cast[DType.uint8](), special.cast[DType.int32](), special.cast[DType.int64](), special.cast[DType.uint64]())
    var edges = SIMD[DType.float64, 4](-9223372036854775808.0, 170141183460469231731687303715884105728.0, -170141183460469231731687303715884105728.0, 4294967296.5)
    print(edges.cast[DType.int64](), edges.cast[DType.uint64](), edges.cast[DType.int32](), edges.cast[DType.uint32](), edges.cast[DType.int16]())
    print((zero / zero).cast[DType.int32](), (zero / zero).cast[DType.float32]())
    var n = SIMD[DType.int, 4](-1, 256, 65536, 4294967296)
    print(n.cast[DType.uint8](), n.cast[DType.int16](), n.cast[DType.uint32](), n.cast[DType.float32]())
