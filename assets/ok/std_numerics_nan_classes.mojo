# `std.utils.numerics`' `nan`, `isnan`, `isinf`, and `isfinite`, re-exported
# by the `std.math` package with `copysign` and `std.math.constants`, are
# upstream's bodies: the classifiers call `llvm.is.fpclass`, which also
# folds at compile time.
from std.math import copysign, e, pi, tau, isnan, isinf, isfinite, nan, inf
from std.math.constants import log2e
from std.math.math import _SQRT2, _LN2, _LN2_HI_F64, _LN2_LO_F64
from std.utils.numerics import neg_inf


def main():
    print(nan[DType.float64](), nan[DType.float32](), nan[DType.float16]())
    var v = SIMD[DType.float32, 4](1, nan[DType.float32](), inf[DType.float32](), neg_inf[DType.float32]())
    print(isnan(v), isinf(v), isfinite(v))
    var h = SIMD[DType.float16, 2](nan[DType.float16](), 2)
    print(isnan(h), isinf(h), isfinite(h))
    print(isnan(Int32(3)), isinf(UInt8(3)), isfinite(Int64(-1)))
    print(isnan(Float64(0.0) / Float64(0.0)))
    comptime folded = isnan(nan[DType.float64]())
    print(folded)
    print(copysign(Float64(3), Float64(-0.0)), copysign(Int32(-3), Int32(2)), copysign(UInt8(3), UInt8(1)))
    print(copysign(SIMD[DType.float32, 2](1, -2), SIMD[DType.float32, 2](-1, 1)))
    print(copysign(Int8(-128), Int8(1)))
    print(e, pi, tau, log2e)
    print(_SQRT2, _LN2, _LN2_HI_F64, _LN2_LO_F64)
