# `**` and `**=` are `SIMD`'s declared `__pow__`, `__rpow__`, and `__ipow__`
# over `_pow`, as at the pin: a float power runs `_powf_scalar`'s
# `exp(log)`, an integer power wraps and takes a negative exponent to 0 or
# ±1, a float literal's `**` is `Float64`'s, and a literal left operand
# takes its default type's `__pow__` before the right operand's `__rpow__`.
def main():
    print(Float64(2) ** Float64(0.5), Float64(2) ** 0.5)
    print(4.0 ** 0.5, 2.0 ** 3, 2 ** 10)
    print(Int32(3) ** 2, Int32(3) ** Int32(2), UInt8(3) ** 5)
    print(Int(2) ** -1, Float64(2) ** -1, Int(-1) ** -3, Int(2) ** 64)
    print(Float64(2) ** Int(3), Float64(-2) ** 0.5)
    var v = SIMD[DType.float32, 4](1, 2, 3, 4)
    var u = SIMD[DType.float64, 2](2, 4)
    print(v ** 2, v ** Float32(2), u ** 0.5, u ** Int32(3))
    var y: Int = 2 ** Int32(3)
    print(y)
    var x = Int(2)
    x **= 3
    var f = Float64(1.5)
    f **= 2
    var w = SIMD[DType.int32, 4](1, 2, 3, 4)
    w **= 3
    print(x, f, w)
    print(Int(2).__pow__(10), Float64(2).__pow__(0.5), Int32(2).__rpow__(3))
    comptime c = Float64(2) ** 0.5
    print(c)
