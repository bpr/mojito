# expect: constraint failed: The element type of the vector must be integer or boolean.
# `SIMD.reduce_and`'s `comptime assert` rejects float lanes in the instance
# the call reaches.
def main():
    var v = SIMD[DType.float32, 2](1, 2)
    print(v.reduce_and())
