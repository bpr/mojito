# expect: type mismatch for variable 's': expected String, found Scalar[dt]
# A `DType`-keyed def is validated once with `Scalar[dt]` symbolic, so the
# invalid arm the only instantiation never selects is still reported.
def kind[dt: DType](a: Scalar[dt]) -> Int:
    comptime if dt == DType.float64:
        return 1
    else:
        var s: String = a
        return 2


def main():
    print(kind[DType.float64](1.5))
