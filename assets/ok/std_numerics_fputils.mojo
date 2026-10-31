# `std.utils.numerics`' `FPUtils` is upstream's struct: its masks and
# accessors are bit arithmetic over `DType`'s float-format queries and
# `bitcast`, and its keyword-only `_constraint` parameter defaults to
# `_constrain_fp_type[dtype]()`, which no member reads. `quiet_nan_mask`
# is not printed: its over-wide shift is poison at the pin.
from std.utils.numerics import FPUtils


def show[dtype: DType](x: Scalar[dtype]):
    comptime F = FPUtils[dtype]
    print(
        F.mantissa_width(),
        F.max_exponent(),
        F.exponent_width(),
        F.exponent_bias(),
    )
    print(
        F.mantissa_mask(),
        F.sign_mask(),
        F.exponent_mask(),
        F.exponent_mantissa_mask(),
    )
    print(F.bitcast_to_integer(x), F.bitcast_to_uint(x))
    print(F.bitcast_from_integer(F.bitcast_to_integer(x)))
    print(F.get_sign(x), F.set_sign(x, True), F.set_sign(-x, False))
    print(
        F.get_exponent(x),
        F.get_exponent_biased(x),
        F.set_exponent(x, F.exponent_bias() + 3),
    )
    print(
        F.get_mantissa(x),
        F.get_mantissa_uint(x),
        F.set_mantissa(x, 1 << (F.mantissa_width() - 1)),
    )
    print(
        F.pack(False, F.exponent_bias(), 0),
        F.pack(True, F.exponent_bias() + 1, 1 << (F.mantissa_width() - 2)),
    )
    print(materialize[F.integral_type](), materialize[F.uint_type]())


def main():
    show(Float64(3.75))
    show(Float32(-0.15625))
    show(Float16(6.5))
    print(
        FPUtils[DType.float64].mantissa_width(),
        FPUtils[DType.float32].exponent_mask(),
        FPUtils[DType.float64].pack(True, 1024, 0),
    )
