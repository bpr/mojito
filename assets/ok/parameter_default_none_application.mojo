# A `NoneType` parameter default that calls a function is an application
# the elaborator evaluates only where the parameter's value is read, as at
# the pin: an instance that never reads it runs even when the call's
# constraint fails.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


def _float_width[dtype: DType]() -> Int:
    comptime assert dtype.is_floating_point(), "dtype must be floating point"
    return 1


def unread[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]() -> Int:
    return 2


def unread_int[dtype: DType, *, _w: Int = _float_width[dtype]()]() -> Int:
    return 3


def read[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]() -> Int:
    print(_c)
    return 4


def main():
    print(unread[DType.float64](), unread[DType.int32]())
    print(unread[DType.int32, _c=_constrain_float[DType.int32]()]())
    print(unread_int[DType.int32]())
    print(read[DType.float32]())
