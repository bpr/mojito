# A type over an application the parameter domain does not express is its
# own type: `h(n)` is not the `n * 2` it computes. The pin keeps the
# application symbolic through the check, so the two widths differ.

def h(x: Int) -> Int:
    return x * 2

def f[n: Int]():
    var a: SIMD[DType.int32, h(n)] = SIMD[DType.int32, n * 2](1)
    print(a)

def main():
    f[2]()
