# `_integral_type_of` and `_unsigned_integral_type_of` are upstream's
# library bodies: the integral dtype of a lane's bit width.
from std.builtin.dtype import _integral_type_of, _unsigned_integral_type_of


def main():
    comptime a = _integral_type_of[DType.float32]()
    comptime b = _unsigned_integral_type_of[DType.int64]()
    comptime c = _unsigned_integral_type_of[DType.float64]()
    comptime d = _integral_type_of[DType.float16]()
    comptime e = _unsigned_integral_type_of[DType.uint8]()
    comptime f = _unsigned_integral_type_of[DType.float16]()
    print(a, b, c, d, e, f)
