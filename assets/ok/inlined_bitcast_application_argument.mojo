# A type argument holding a compile-time application is answered before
# the instance it is passed to is keyed: `bitcast`'s inlined width assert
# demands `bit_width_of[SIMD[dtype, width]]` with `dtype` an application.
from std.memory import bitcast
from std.builtin.dtype import _unsigned_integral_type_of, _integral_type_of


def g[dt: DType](x: Scalar[dt]) -> Scalar[_integral_type_of[dt]()]:
    return bitcast[_integral_type_of[dt](), 1](x)


def main():
    comptime uint = _unsigned_integral_type_of[DType.int64]()
    print(bitcast[uint, 1](Int64(-1)))
    print(g(Float64(-0.0)))
