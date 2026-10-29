# expect: constraint failed: the SIMD type must be numeric
# `SIMD.__ifloordiv__`'s `comptime assert` rejects a `bool` mask in the
# instance an augmented `//=` reaches.
def main():
    var m = SIMD[DType.bool, 4](fill=True)
    m //= m
    print(m)
