# An application of a function with a collector is not the value it
# computes: `SIMD[DType.int8, 2]` is not `SIMD[DType.int8, total(1, 1)]`,
# as at the pin.
# expect: type mismatch for argument to 'f'
def total(*xs: Int) -> Int:
    var s = 0
    for x in xs:
        s += x
    return s


def f[n: Int](
    x: SIMD[DType.int8, total(n, 1)]
) -> SIMD[DType.int8, total(n, 1)]:
    return x


def main():
    print(f[1](SIMD[DType.int8, 2](1)))
