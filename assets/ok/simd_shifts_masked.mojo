# Shift counts mask to the lane width (`count & (bits - 1)`): counts at or
# beyond the width and negative counts wrap, `>>` is arithmetic on signed
# lanes and logical on unsigned lanes.
def main():
    var i8 = SIMD[DType.int8, 4](-128, 64, -1, 1)
    print(i8 << 1, i8 << 8, i8 << 9, i8 << -1, i8 >> 1, i8 >> 8, i8 >> -1, i8 >> 7)
    var u8 = SIMD[DType.uint8, 4](128, 64, 255, 1)
    print(u8 << 1, u8 << 9, u8 >> 1, u8 >> 9, u8 >> 7, u8 >> -1)
    var i16 = SIMD[DType.int16, 2](-32768, 1)
    print(i16 << 15, i16 << 17, i16 >> 15, i16 >> 16)
    var u16 = SIMD[DType.uint16, 2](32768, 65535)
    print(u16 << 1, u16 >> 15, u16 >> 17)
    var i32 = SIMD[DType.int32, 2](-2147483648, 1)
    print(i32 << 31, i32 << 33, i32 >> 31, i32 >> 32)
    var u32 = SIMD[DType.uint32, 2](2147483648, 4294967295)
    print(u32 << 1, u32 >> 31, u32 >> 33)
    var i64 = SIMD[DType.int64, 2](-9223372036854775807 - 1, 1)
    print(i64 << 63, i64 << 65, i64 >> 63, i64 >> 64)
    var u64 = SIMD[DType.uint64, 2](9223372036854775808, 18446744073709551615)
    print(u64 << 1, u64 >> 63, u64 >> 65)
    var counts = SIMD[DType.int32, 4](0, 1, 31, 32)
    print(SIMD[DType.int32, 4](1) << counts, SIMD[DType.int32, 4](-2147483648) >> counts)
    var n = SIMD[DType.int, 2](1, -8)
    print(n << 62, n << 64, n >> 1, n >> 65)
