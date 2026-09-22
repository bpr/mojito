# expect: operator '*' is not defined for Scalar[dt] and Int
# A concrete scalar never splats into a symbolic lane: `Int(2)` does not
# convert to `Scalar[dt]` (nor does `Int32(1)` or a `Bool`), where an
# integer or float literal does.
def scaled[dt: DType](a: Scalar[dt]) -> Scalar[dt]:
    comptime if dt == DType.int32:
        return a * Int(2)
    return a


def main():
    pass
