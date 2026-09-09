# Widths 8 and 16 (wider than one physical register): arithmetic, shuffles
# that widen and narrow, lane reads at both ends, and reductions.
def main():
    var a = SIMD[DType.int32, 8](1, 2, 3, 4, 5, 6, 7, 8)
    var b = a * a + a
    print(b, b[0], b[7], b.reduce_add(), b.reduce_max())
    var c = SIMD[DType.uint8, 16](250, 251, 252, 253, 254, 255, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9)
    var d = c + 10
    print(d, d.reduce_add(), d.reduce_min(), d[5], d[15])
    var e = SIMD[DType.float32, 16](0.5)
    var f = e * SIMD[DType.float32, 16](1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0)
    print(f, f.reduce_add(), f.reduce_mul(), f[15])
    print(a.shuffle[7, 6, 5, 4, 3, 2, 1, 0](), a.shuffle[0, 0, 7, 7](), a.shuffle[1, 3, 5, 7, 1, 3, 5, 7, 0, 2, 4, 6, 0, 2, 4, 6]())
    print(c.shuffle[15, 0](), c.shuffle[5, 6, 7, 8, 9, 10, 11, 12]())
    var m = d > 5
    print(m, m.reduce_and(), m.reduce_or(), m.select(d, 0))
    var w = SIMD[DType.int64, 16](1) << SIMD[DType.int64, 16](0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60)
    print(w, w.reduce_add())
