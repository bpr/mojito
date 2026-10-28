# `SIMD.__int__` is its declared member, whose `comptime assert` fails a
# vector instance, as at the pin.
# expect: constraint failed: expected a scalar type
def main():
    print(SIMD[DType.int32, 4](1, 2, 3, 4).__int__())
