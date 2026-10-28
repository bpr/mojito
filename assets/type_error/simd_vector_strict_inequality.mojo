# expect: function instantiation of `main` failed: constraint failed: Strict inequality is only defined for `Scalar`s; did you mean to use `SIMD.lt(...)`?
# Upstream's `SIMD.__lt__` asserts one lane, so `<` over two four-lane
# vectors fails the instance that reaches it; `SIMD.lt` is the mask.

def main():
    var v = SIMD[DType.int32, 4](1, 2, 3, 4)
    var w = SIMD[DType.int32, 4](4, 3, 2, 1)
    print(v < w)
