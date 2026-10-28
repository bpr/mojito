# `SIMD.fma` and `std.math.fma` are upstream's library bodies over
# `pop.fma`, whose `fastmathFlags` is the `FastMathFlag` parameter's
# computed `_mlir_attr()`; a float range steps through `fma` too.
from std.math import fma
from std.simd import FastMathFlag


def flag_name(flag: FastMathFlag) -> String:
    match flag:
        case .CONTRACT:
            return "contract"
        case .FAST:
            return "fast"
        case _:
            return "other"


def scaled[flag: FastMathFlag](x: Float64) -> Float64:
    return x.fma[flag](2.0, 0.5)


def main():
    var v = Float32(2.0)
    print(v.fma(Float32(3.0), Float32(1.0)))
    print(v.fma[FastMathFlag.NONE](3.0, 1.0), v.fma[FastMathFlag.FAST](3.0, 1.0))
    var w = SIMD[DType.float64, 4](1.0, 2.0, 3.0, 4.0)
    print(w.fma(w, SIMD[DType.float64, 4](0.5)))
    print(fma(Float64(0.1), Float64(10.0), Float64(-1.0)))
    print(Float16(1.5).fma(Float16(2.0), Float16(0.25)))
    print(Int32(7).fma(Int32(6), Int32(-2)), fma(Int(3), Int(4), Int(5)))
    print(UInt8(200).fma(UInt8(2), UInt8(1)))
    print(scaled[FastMathFlag.REASSOC](1.25), scaled[FastMathFlag.NNAN](-3.0))
    print(FastMathFlag.CONTRACT == FastMathFlag.CONTRACT, FastMathFlag.NONE == FastMathFlag.FAST)
    print(flag_name(FastMathFlag.CONTRACT), flag_name(FastMathFlag.FAST), flag_name(FastMathFlag.AFN))
    for x in range(Float64(0.0), Float64(1.0), Float64(0.25)):
        print(x)
    for y in range(Float32(1.0), Float32(0.0), Float32(-0.5)):
        print(y)
