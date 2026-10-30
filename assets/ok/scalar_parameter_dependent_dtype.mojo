# A value parameter typed by an earlier binder (`c: Scalar[dtype]`), bound
# from a literal, a vector construction, or one spelled through `Scalar`,
# closed or symbolic in the dtype.


def get[dtype: DType, c: Scalar[dtype]]() -> Scalar[dtype]:
    return c


def from_literal[dtype: DType]() -> Scalar[dtype]:
    return get[dtype, 2.0]()


def from_construction[dtype: DType]() -> Scalar[dtype]:
    return get[dtype, Scalar[dtype](-3)]() + get[dtype, SIMD[dtype, 1](0.5)]()


def main():
    print(get[DType.float32, 2.5](), get[DType.int16, Int16(7)]())
    print(get[DType.float32, Scalar[DType.float32](1.5)]())
    print(from_literal[DType.float64](), from_construction[DType.float32]())
