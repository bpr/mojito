# expect: constraint failed: the type must be numeric
# `SIMD.__floordiv__`'s `comptime assert` rejects a `bool` mask.
def main():
    var m = SIMD[DType.bool, 4](fill=True)
    print(m // m)
