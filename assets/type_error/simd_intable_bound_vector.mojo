# A vector is `Intable`, since `SIMD` declares the trait at every width, so
# the bound admits it; the declared `__int__`'s `comptime assert` then fails
# the instance, as at the pin.
# expect: constraint failed: expected a scalar type
def to_int[T: Intable](value: T) -> Int:
    return value.__int__()


def main():
    print(to_int(SIMD[DType.int32, 2](1, 2)))
