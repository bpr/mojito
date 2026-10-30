# A force-inlined generic whose untaken `comptime if` arm returns a float
# literal as `Scalar[dtype]` runs at an integral dtype: the pin judges the
# literal's conversion only in the instance that takes the arm.


@always_inline
def pick[dtype: DType]() -> Scalar[dtype]:
    comptime if dtype.is_integral():
        return 1
    elif dtype == .float32:
        return 2.5
    else:
        return 0


@inline(.nodebug)
def half[dtype: DType]() -> Scalar[dtype]:
    comptime if dtype.is_floating_point():
        return 0.5
    else:
        return 2


def main():
    print(pick[DType.int8](), pick[DType.float32](), pick[DType.int64]())
    print(half[DType.uint16](), half[DType.float64]())
