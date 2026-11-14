# A function parameter's default that calls a function keys the instance by
# the application, and a read the callee drops as an unused receiver never
# evaluates it, as at the pin: `NoneType.write_to` removes its receiver.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


def written[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]() -> Int:
    var s = String()
    _c.write_to(s)
    print(s)
    return 3


def forwarded[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]() -> Int:
    var s = String()
    NoneType.write_to(_c, s)
    return 4


def main():
    print(written[DType.int32]())
    print(forwarded[DType.int32]())
