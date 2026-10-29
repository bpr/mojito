# expect: constraint failed: the SIMD type must be numeric
# `SIMD.__neg__`'s `comptime assert Self.dtype.is_numeric()` rejects a
# `bool` mask in the instance a run-time prefix `-` reaches.
def main():
    var m = SIMD[DType.int32, 2](1, 2).lt(SIMD[DType.int32, 2](2, 2))
    var bad = -m
    print(bad)
