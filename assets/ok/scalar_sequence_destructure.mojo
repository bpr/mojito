# A scalar or vector destructures as a sequence through `SIMD`'s `__len__`
# and `__getitem__`, one element per lane, and a pattern of the wrong length
# raises before any store.
def main() raises:
    var [a] = 5
    print(a)
    var [b, c] = SIMD[DType.int32, 2](1, 2)
    print(b, c)
    var [g] = 2.5
    print(g)
    var v = SIMD[DType.uint8, 4](9, 8, 7, 6)
    var [w, x, y, z] = v
    print(w, x, y, z)
    try:
        var [d, e] = 5
        print(d, e)
    except err:
        print("error:", err)
