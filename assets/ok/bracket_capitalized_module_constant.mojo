# A capitalized module constant's element in a value parameter's bracket.

comptime XS = [1, 4, 5]
comptime T = (2, 3)
comptime xs = [1, 4, 5]


def show[n: Int]():
    print(n)


def main():
    show[XS[1]]()
    show[T[1]]()
    show[xs[1]]()
    print(SIMD[DType.int64, XS[1]](7))
