# `SIMD`'s declared `reduce_and`, `reduce_or`, and `reduce_bit_count`: one
# lane by default or `size_out` lanes, over `bool` masks and integer lanes,
# at run time and at compile time.
def main():
    var m = SIMD[DType.bool, 4](True, False, True, True)
    print(m.reduce_and(), m.reduce_or(), m.reduce_and[2]())
    var v = SIMD[DType.int32, 4](7, 5, 13, 15)
    print(v.reduce_and(), v.reduce_or(), v.reduce_or[2](), v.reduce_bit_count())
    var x: Bool = m.reduce_and()
    print(x, 3 in v, 5 in v)
    if m.reduce_or() and v.reduce_and():
        print("yes")
    print(SIMD[DType.bool, 8](True, False, True, True, False, False, True, True).reduce_bit_count())
    print(Scalar[DType.bool](True).reduce_bit_count(), Int32(7).reduce_and())
    var s = m.reduce_or() or m.reduce_and()
    print(s)
    comptime a = SIMD[DType.bool, 4](True, True, False, True).reduce_and()
    comptime o = SIMD[DType.bool, 4](True, True, False, True).reduce_or()
    comptime i = SIMD[DType.int8, 4](7, 3, 3, 3).reduce_and()
    comptime c = SIMD[DType.int8, 4](7, 3, 3, 3).reduce_bit_count()
    comptime d = SIMD[DType.bool, 8](True, True, False, True, True, True, False, True).reduce_bit_count()
    print(a, o, i, c, d)
    comptime assert all(SIMD[DType.int32, 2](1, 2))
    print(any(SIMD[DType.float32, 2](0, 0)))
    print(SIMD[DType.bool, 16](fill=True).reduce_bit_count())
