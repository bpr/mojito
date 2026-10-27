# A capitalized subscript continued by an operator is one bracket operand.

comptime T = (2, 3)


def f[n: Int]():
    comptime L = [n, n * 2]
    print(SIMD[DType.int32, L[0] * 2](5))


def g[*Ts: Movable]():
    print(materialize[Ts[0] == Int]())


def main():
    f[4]()
    g[Int, Bool]()
    print(SIMD[DType.int64, T[0] * 2](1))
