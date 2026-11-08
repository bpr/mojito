# expect: (load_from_mem Array[Int, 2](2, 1, None)[0])
# An explicit `__getitem__` call in a type argument is `load_from_mem` of
# its application, not the element it computes, so the parameter is not
# `2`, as at the pin.
def f(x: SIMD[DType.int8, [2, 1].__getitem__(0)]) -> SIMD[DType.int8, 2]:
    return x


def main():
    print(f(SIMD[DType.int8, 2](1, 2)))
