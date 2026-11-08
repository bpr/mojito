# expect: .__getitem_param__[0]())]
# A tuple display's subscript in a type argument is `load_from_mem` of its
# `__getitem_param__` application, not the element it computes, so the
# parameter is not `2`, as at the pin.
def f(x: SIMD[DType.int8, (2, 1)[0]]) -> SIMD[DType.int8, 2]:
    return x


def main():
    print(f(SIMD[DType.int8, 2](1, 2)))
