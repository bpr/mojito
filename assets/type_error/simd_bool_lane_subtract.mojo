# expect: constraint failed: the SIMD type must be numeric
# `SIMD.__sub__`'s `comptime assert Self.dtype.is_numeric()` rejects a
# `bool` mask in the instance a run-time `-` reaches.
def main():
    var b = SIMD[DType.bool, 2](True, False)
    print(b - b)
