# expect: type mismatch for variable 's': expected String, found Scalar[dt]
# A `DType`-keyed struct applied at a body's own symbolic `dt` binds its
# `Scalar[Self.dt]` constructor parameter and `get` result to the caller's
# lane, so the invalid untaken arm over the instance is reported.
struct Vec[dt: DType](Movable):
    var x: Scalar[Self.dt]

    def __init__(out self, x: Scalar[Self.dt]):
        self.x = x

    def get(self) -> Scalar[Self.dt]:
        return self.x


def make[dt: DType](a: Scalar[dt]) -> Vec[dt]:
    var v = Vec[dt](a)
    comptime if dt == DType.float64:
        return v^
    else:
        var s: String = v.get()
        return v^


def main():
    print(make[DType.float64](2.5).get())
