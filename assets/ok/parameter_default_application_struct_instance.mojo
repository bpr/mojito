# A struct parameter's default that calls a function keys the instance by
# the application, unevaluated, as at the pin: constructing the instance
# and calling a method that never reads the parameter run even when the
# call's constraint fails, and a read of a passing one is its value.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


def _float_width[dtype: DType]() -> Int:
    comptime assert dtype.is_floating_point(), "dtype must be floating point"
    return 1


def _width[dtype: DType]() -> Int:
    return 7


struct Checked[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return 1


struct Wide[dtype: DType, *, _w: Int = _float_width[dtype]()]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return 2


struct Read[dtype: DType, *, w: Int = _width[dtype]()]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.w


def take(r: Read[DType.int32]) -> Int:
    return r.get()


def main():
    _ = Checked[DType.int32]()
    var checked = Checked[DType.int32]()
    print(checked.get())
    var wide = Wide[DType.int32]()
    print(wide.get())
    var read = Read[DType.int32]()
    print(read.get(), take(read), Read[DType.float32]().get())
