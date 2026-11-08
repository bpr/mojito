# The target-independent elementwise LLVM intrinsics upstream's `math`,
# `base64`, and `sys.intrinsics` call: the float family of `math`'s
# `_llvm_unary_fn` (`sin`, `cos`, `asin`, `acos`, and their kin) over
# `float64`, `float32`, and `float16` lanes, the two- and three-operand
# float intrinsics, the saturating and min/max integer intrinsics of
# `base64`'s `_subs`, `llvm.abs`, and `llvm.expect` over an integer and
# a `Bool`, at run time, and those LLVM's constant folder folds at compile
# time.
from std.sys import llvm_intrinsic


def unary[name: StaticString](x: SIMD) -> type_of(x):
    return llvm_intrinsic[name, type_of(x), has_side_effect=False](x)


def binary[name: StaticString](x: SIMD, y: type_of(x)) -> type_of(x):
    return llvm_intrinsic[name, type_of(x), has_side_effect=False](x, y)


def ternary[
    name: StaticString
](x: SIMD, y: type_of(x), z: type_of(x)) -> type_of(x):
    return llvm_intrinsic[name, type_of(x), has_side_effect=False](x, y, z)


def main():
    var x = Float64(0.5)
    print(unary["llvm.sin"](x), unary["llvm.cos"](x), unary["llvm.tan"](x))
    print(unary["llvm.asin"](x), unary["llvm.acos"](x), unary["llvm.atan"](x))
    print(unary["llvm.sinh"](x), unary["llvm.cosh"](x), unary["llvm.tanh"](x))
    print(unary["llvm.exp"](x), unary["llvm.exp2"](x), unary["llvm.log"](x))
    print(unary["llvm.log2"](x), unary["llvm.log10"](x))

    var r = SIMD[DType.float64, 4](-2.5, -0.5, 1.5, 2.5)
    print(unary["llvm.fabs"](r), unary["llvm.floor"](r), unary["llvm.ceil"](r))
    print(unary["llvm.trunc"](r), unary["llvm.round"](r))
    print(unary["llvm.roundeven"](r), unary["llvm.rint"](r))
    print(unary["llvm.nearbyint"](r))

    var f = SIMD[DType.float32, 4](0.1, 1.0, 2.5, -3.75)
    # Widened to `float64` to print, since Mojito does not yet print a
    # `float32` lane at its own precision (roadmap R159).
    comptime wide = DType.float64
    print(unary["llvm.sin"](f).cast[wide](), unary["llvm.exp"](f).cast[wide]())
    print(unary["llvm.cos"](f).cast[wide](), unary["llvm.atan"](f).cast[wide]())
    var g = SIMD[DType.float32, 4](2.0, 0.5, 3.0, 1.5)
    print(
        binary["llvm.pow"](g, f).cast[wide](),
        binary["llvm.atan2"](f, g).cast[wide](),
    )
    print(
        ternary["llvm.fma"](f, g, f).cast[wide](),
        ternary["llvm.fmuladd"](g, g, f).cast[wide](),
    )
    var h = SIMD[DType.float16, 2](0.5, 2.0)
    print(unary["llvm.sin"](h).cast[wide](), unary["llvm.log2"](h).cast[wide]())

    var q = Float64(0.0)
    var n = q / q
    var nan = SIMD[DType.float64, 4](n, -0.5, 1.0, 2.0)
    var other = SIMD[DType.float64, 4](1.0, 0.5, n, -3.0)
    print(binary["llvm.minnum"](nan, other), binary["llvm.maxnum"](nan, other))
    var zero = SIMD[DType.float64, 4](n, -0.0, 1.0, 0.0)
    var signed = SIMD[DType.float64, 4](1.0, 0.0, n, -0.0)
    print(
        binary["llvm.minimum"](zero, signed),
        binary["llvm.maximum"](zero, signed),
    )

    var a = SIMD[DType.uint8, 4](10, 200, 3, 255)
    var b = SIMD[DType.uint8, 4](20, 100, 3, 1)
    print(binary["llvm.usub.sat"](a, b), binary["llvm.uadd.sat"](a, b))
    var i = SIMD[DType.int8, 4](-100, 100, -128, 5)
    var j = SIMD[DType.int8, 4](-100, 100, 1, -7)
    print(binary["llvm.sadd.sat"](i, j), binary["llvm.ssub.sat"](i, j))
    print(binary["llvm.smax"](i, j), binary["llvm.smin"](i, j))
    print(binary["llvm.umax"](i, j), binary["llvm.umin"](i, j))
    print(llvm_intrinsic["llvm.abs", type_of(i)](i, False))
    print(llvm_intrinsic["llvm.abs", Int64](Int64(-7), True))

    print(llvm_intrinsic["llvm.expect", Int, has_side_effect=False](5, 3))
    print(llvm_intrinsic["llvm.expect", Bool, has_side_effect=False](True, False))

    comptime folded = (
        unary["llvm.sin"](Float64(0.5)),
        unary["llvm.floor"](SIMD[DType.float32, 2](1.5, -1.5)),
        binary["llvm.pow"](Float64(2.0), Float64(10.0)),
        binary["llvm.usub.sat"](UInt8(3), UInt8(5)),
        binary["llvm.smax"](Int32(-3), Int32(2)),
    )
    print(folded[0], folded[1], folded[2], folded[3], folded[4])
