# Float-to-int lane conversion on both sides of the exactly-convertible
# range. Lanes whose magnitude stays inside it convert as a whole vector;
# a lane at or past the boundary sends the whole vector down the per-lane
# path instead, so the same vector can cross the boundary for one target
# width and stay inside it for another. Every result here fits its target,
# so the two paths are compared where the conversion is defined rather than
# where it wraps.
def main():
    var under31 = SIMD[DType.float32, 4](2000000000.0, 1.5, 65535.5, 0.0)
    print(under31.cast[DType.uint32](), under31.cast[DType.int32](), under31.cast[DType.int64](), under31.cast[DType.uint64]())
    var over31 = SIMD[DType.float32, 4](3000000000.0, 1.5, 65535.5, 0.0)
    print(over31.cast[DType.uint32](), over31.cast[DType.int64](), over31.cast[DType.uint64]())
    var edge31 = SIMD[DType.float64, 4](2147483647.0, 2147483648.0, 4294967295.0, 0.0)
    print(edge31.cast[DType.int64](), edge31.cast[DType.uint32](), edge31.cast[DType.uint64]())
    var under63 = SIMD[DType.float64, 4](9223372036854774784.0, 2.5, 1024.5, 0.0)
    print(under63.cast[DType.int64](), under63.cast[DType.uint64]())
    var over63 = SIMD[DType.float64, 4](15000000000000000000.0, 2.5, 1024.5, 0.0)
    print(over63.cast[DType.uint64]())
    var edge63 = SIMD[DType.float64, 2](9223372036854775808.0, 9223372036854774784.0)
    print(edge63.cast[DType.uint64]())
    var narrow = SIMD[DType.float32, 8](0.5, -0.5, 127.9, -128.9, 100.5, -1.5, 1000.25, -1000.25)
    print(narrow.cast[DType.int32](), narrow.cast[DType.int16](), narrow.cast[DType.int64]())
    var wide = SIMD[DType.float64, 4](3.7, -3.7, 2147483647.0, -2147483647.0)
    print(wide.cast[DType.int32](), wide.cast[DType.int64]())
