# A struct's `NoneType` parameter default that calls a function, as
# upstream's `FPUtils` declares `_constraint`: a static method or a
# `comptime` member of an instance that never reads it runs even when the
# call's constraint fails, as at the pin.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


struct Fp[dtype: DType, *, _constraint: NoneType = _constrain_float[dtype]()]:
    comptime width = 7

    @staticmethod
    def bits() -> Int:
        return 1

    @staticmethod
    def checked() -> Int:
        print(Self._constraint)
        return 2


struct Unit[k: Int, *, _x: NoneType = _constrain_float[DType.float16]()]:
    @staticmethod
    def get() -> Int:
        return Self.k


def main():
    print(Fp[DType.float64].width, Fp[DType.int32].width)
    print(Fp[DType.float64].bits(), Fp[DType.int32].bits())
    print(Fp[DType.float32].checked())
    print(Unit[4].get())
