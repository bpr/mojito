# expect: violated constraint
# A `where` type equality over a value parameter that the call's arguments
# make false (`Int == Scalar[DType.int32]`) is a violated constraint, not
# one lacking evidence.
def over_dtype[T: AnyType, dtype: DType]() -> Int where (T == Scalar[dtype]):
    return 1


def main():
    print(over_dtype[Int, DType.int32]())
