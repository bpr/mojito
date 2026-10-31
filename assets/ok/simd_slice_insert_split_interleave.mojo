# `SIMD`'s `slice`, `insert`, `split`, `interleave`, and `deinterleave` are
# upstream's declared members: `slice` reads lanes in an element loop, or
# through `llvm.vector.extract` at a native-width offset; `insert` writes
# them; `split` is two slices; and `interleave` and `deinterleave` are
# `llvm.vector.interleave2` and `llvm.vector.deinterleave2`, with their
# width-1 and width-2 arms. Each matches the pin.
def halves[dt: DType, w: Int](v: SIMD[dt, w]):
    var lo, hi = v.split()
    print(lo, hi)


def main():
    var v = SIMD[DType.int32, 4](10, 20, 30, 40)
    print(v.slice[2](), v.slice[2, offset=2](), v.slice[1, offset=3]())
    print(v.insert[offset=2](SIMD[DType.int32, 2](7, 8)))
    print(v.insert[offset=1](Int32(9)))
    var a, b = v.split()
    print(a, b)
    print(v.interleave(SIMD[DType.int32, 4](1, 2, 3, 4)))
    var e, o = v.deinterleave()
    print(e, o)
    print(Int32(3).interleave(Int32(4)))
    var p, q = SIMD[DType.int32, 2](5, 6).deinterleave()
    print(p, q)
    var f = SIMD[DType.float32, 8](1, 2, 3, 4, 5, 6, 7, 8)
    print(f.slice[4, offset=4](), f.slice[2, offset=2](), f.slice[8]())
    var x, y = f.split()
    print(x, y)
    halves[DType.uint8, 8](SIMD[DType.uint8, 8](1, 2, 3, 4, 5, 6, 7, 8))
