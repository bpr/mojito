# expect: function instantiation of `_constrain_float` failed: constraint failed: dtype must be floating point
# A `NoneType` parameter default that calls a function is evaluated where
# the instance reads the parameter, and its failing constraint rejects the
# program, as at the pin.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


def read[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]() -> Int:
    print(_c)
    return 4


def main():
    print(read[DType.int32]())
