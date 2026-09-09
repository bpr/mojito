# Integer `//` and `%` floor toward negative infinity per lane, a zero
# divisor lane yields 0 (no trap), and `MIN // -1` wraps back to `MIN`.
def main():
    var a = SIMD[DType.int8, 4](7, -7, 7, -7)
    var b = SIMD[DType.int8, 4](2, 2, -2, -2)
    print(a // b, a % b)
    print(a // SIMD[DType.int8, 4](0), a % SIMD[DType.int8, 4](0))
    var edge = SIMD[DType.int8, 4](-128, -128, 127, -1)
    print(edge // SIMD[DType.int8, 4](-1, 0, -1, 0), edge % SIMD[DType.int8, 4](-1, 0, -1, 0))
    var u = SIMD[DType.uint8, 4](255, 200, 7, 0)
    print(u // 3, u % 3, u // SIMD[DType.uint8, 4](0), u % 0)
    var i32 = SIMD[DType.int32, 4](-2147483648, 2147483647, -5, 5)
    print(i32 // -1, i32 % -1, i32 // 3, i32 % 3, i32 // -3, i32 % -3)
    var i64 = SIMD[DType.int64, 2](-9223372036854775807 - 1, 9223372036854775807)
    print(i64 // -1, i64 % -1, i64 // 7, i64 % 7, i64 // 0, i64 % 0)
    var u64 = SIMD[DType.uint64, 2](18446744073709551615, 10)
    print(u64 // 7, u64 % 7, u64 // 0, 100 // SIMD[DType.uint64, 2](7, 0), 100 % SIMD[DType.uint64, 2](7, 0))
    var n = SIMD[DType.int, 4](-7, 7, -9223372036854775807 - 1, 0)
    print(n // 2, n % 2, n // -1, n % -1, n // 0, n % 0)
