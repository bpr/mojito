# expect: constraint failed: dtype must be floating point
# A float-format query over a `DType` binder fails the instantiation at a
# non-float dtype, through the query's own floating-point constraint.
def mantissa_of[dt: DType]() -> Int:
    return DType.mantissa_width[dt]()


def main():
    print(mantissa_of[DType.int32]())
