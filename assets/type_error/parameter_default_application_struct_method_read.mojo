# expect: function instantiation of `_constrain_float` failed: constraint failed: dtype must be floating point
# A struct's parameter default that calls a function keys the instance
# unevaluated, and a method of a constructed instance that reads the
# parameter evaluates it: its failing constraint rejects the program, as at
# the pin.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


struct Checked[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        print(Self._c)
        return 1


def main():
    var checked = Checked[DType.int32]()
    print(checked.get())
