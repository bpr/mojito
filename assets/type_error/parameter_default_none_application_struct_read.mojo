# expect: function instantiation of `_constrain_float` failed: constraint failed: dtype must be floating point
# A struct's `NoneType` parameter default that calls a function is
# evaluated where a method of the instance reads the parameter, and its
# failing constraint rejects the program, as at the pin.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


struct Fp[dtype: DType, *, _constraint: NoneType = _constrain_float[dtype]()]:
    @staticmethod
    def checked() -> Int:
        print(Self._constraint)
        return 2


def main():
    print(Fp[DType.int32].checked())
