# expect: constraint failed: the SIMD type must be numeric
# `SIMD.__iadd__`'s `comptime assert` rejects a `bool` mask in the instance
# an augmented `+=` reaches.
def main():
    var m = SIMD[DType.bool, 2](True, False)
    m += m
    print(m)
