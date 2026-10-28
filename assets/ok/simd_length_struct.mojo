# `SIMDLength` is upstream's library struct: a `[w: SIMDLength]` binder is a
# struct-typed value parameter, a vector's width slot holds its lane count,
# `v.length` and `T.length` are the `SIMDLength` parameter, and a
# `SIMDLength` converts to any vector through `SIMD`'s `@implicit
# __init__(SIMDLength)`, beside a vector operand, after a literal, and by
# name (`Int(w)`).
struct Tag[w: SIMDLength]:
    def __init__(out self):
        pass


def tens[w: SIMDLength]() -> Int:
    return Int(w) * 10


def lanes[w: SIMDLength](x: SIMD[DType.int32, w]) -> Int:
    return Int(w)


def over[m: SIMDLength]():
    var a = SIMD[DType.int32, Int(m) * 2](1)
    print(lanes(a), a)


def show[w: SIMDLength]():
    print(Int(w), w.is_power_of_two(), Int(w >> 1), Int(w**2), Int(-w))
    var v = SIMD[DType.int8, w * 2](1)
    print(v, 2 * w, tens[w + 1]())
    var t = 0
    for i in range(w):
        t += i
    var u: UInt = UInt(w)
    var f: Float32 = w
    print(t, u, f, Scalar[DType.int16](w))
    comptime k = 2 * w
    var c: SIMD[DType.int8, k] = SIMD[DType.int8, w * 2](3)
    print(c, Int(k))


def main():
    var t: Tag[SIMDLength(1) + SIMDLength(2)] = Tag[SIMDLength(3)]()
    _ = t^
    show[4]()
    over[2]()
    var v = SIMD[DType.int8, 4](1)
    comptime L = v.length
    print(Int(L), Int(v.length) * 2, v.length == 4, v.length > 2)
    var x: Int = v.length
    var s: SIMD[DType.float32, 4] = v.length
    print(x, s, 2 * v.length, v.length * 2 == 8)
    comptime T = SIMD[DType.int8, 8]
    var y: Int = T.length
    print(y, Int(Int32.length))
    var a: SIMDLength = 3
    var b: SIMDLength = 4
    print(a < b, a == b, Int(a + b), Int(b - a), Int(a * b), Int(b / a))
    print(Int(a & b), Int(b >> SIMDLength(1)), Int(a ** SIMDLength(2)))
    var c = a
    c *= b
    print(Int(c))
