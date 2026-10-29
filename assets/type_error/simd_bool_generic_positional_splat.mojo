# expect: must be a scalar; use the `fill` keyword
# The `Bool` initializer's `comptime assert Self.length == 1` is judged per
# instance: a mask of symbolic width takes a positional `Bool` only where
# the instantiation is one lane wide.
def one_lane[n: Int](b: Bool) -> SIMD[DType.bool, n]:
    return SIMD[DType.bool, n](b)

def main():
    print(one_lane[4](True))
