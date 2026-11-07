# expect: not an associated comptime expression
# A display's element in a type argument is the display's subscript
# application, not the element it computes, so the parameter is not `2`, as
# at the pin.
def f(x: SIMD[DType.int8, [2, 1][0]]) -> SIMD[DType.int8, 2]:
    return x


def main():
    print(f(SIMD[DType.int8, 2](1, 2)))
