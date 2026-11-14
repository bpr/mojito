# A compile-time application of a static method keys its owner instance by
# the owner's default application, unevaluated, as at the pin: a default
# whose constraint fails is never run when the method does not read it.


def _constrain_float[dtype: DType]():
    comptime assert dtype.is_floating_point(), "dtype must be floating point"


struct Checked[dtype: DType, *, _c: NoneType = _constrain_float[dtype]()]:
    @staticmethod
    def one() -> Int:
        return 1


def main():
    comptime x = Checked[DType.int32].one()
    print(x)
