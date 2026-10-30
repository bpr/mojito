# expect: constraint failed: the SIMD type must be numeric
# `/` over a `bool` mask is `SIMD.__truediv__`, whose `comptime assert`
# rejects a non-numeric lane, as the pin's does (`simd.mojo:1155`).
def main():
    var m = SIMD[DType.bool, 2](True, False)
    print(m / m)
