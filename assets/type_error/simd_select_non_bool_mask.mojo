# `SIMD.select` is its declared member, whose `comptime assert` fails a
# non-`bool` mask, as at the pin.
# expect: constraint failed: the simd type must be bool
def main():
    var v = SIMD[DType.int32, 2](1, 0)
    print(v.select(v, v))
