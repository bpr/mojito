# expect: found SIMD[DType.int8, (load_from_mem mk()[0])]
# A call result's subscript in a type argument is `load_from_mem` of the
# getter's application, not the element it computes, so the parameter is
# not `2`, as at the pin.
def mk() -> List[Int]:
    return [2, 1]


def f(x: SIMD[DType.int8, mk()[0]]) -> SIMD[DType.int8, 2]:
    return x


def main():
    print(f(SIMD[DType.int8, 2](1, 2)))
