# A local `comptime` tuple binding's element read is its constructor's
# application, so `h(t[0])` is not `h(n)` in a type, as at the pin.


def h(x: Int) -> Int:
    return x + 1


def f[n: Int]():
    comptime t = (n, n * 2)
    var a: SIMD[DType.int32, h(t[0])] = SIMD[DType.int32, h(n)](1)
    print(a)


def main():
    f[3]()
