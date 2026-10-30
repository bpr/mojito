# expect: constraint failed: the SIMD type must be numeric
# `/=` over a `bool` mask is `SIMD.__itruediv__`, whose `comptime assert`
# rejects a non-numeric lane, as the pin's does (`simd.mojo:1718`).
def main():
    var m = SIMD[DType.bool, 2](True, False)
    m /= m
    print(m)
