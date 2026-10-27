# A call of an `@always_inline("builtin")` function in a parameter
# expression is its body's expression, so `SIMD[DType.int8, twice(n)]` is
# `SIMD[DType.int8, n + n]`, as the pinned Mojo folds it.


@always_inline("builtin")
def twice(n: Int) -> Int:
    return n + n


@always_inline("builtin")
def quad(n: Int) -> Int:
    return twice(twice(n))


@always_inline("builtin")
def bump[k: Int](n: Int) -> Int:
    var m = n + 1
    return m + k - 1


@always_inline("builtin")
def pick(n: Int) -> Int:
    return 1 if n > 3 else n


@always_inline("builtin")
def doubled(n: Int) -> Int:
    return Int(mlir_value=__mlir_op.`pop.add`(n._mlir_value, n._mlir_value))


struct Widths:
    @always_inline("builtin")
    @staticmethod
    def twice(n: Int) -> Int:
        return n + n


struct Lanes[n: Int]:
    var lanes: SIMD[DType.int8, twice(Self.n)]

    def __init__(out self):
        self.lanes = SIMD[DType.int8, Self.n + Self.n](1)


def same[n: Int](x: SIMD[DType.int8, twice(n)]) -> SIMD[DType.int8, n + n]:
    return x


def four[n: Int](x: SIMD[DType.int8, quad(n)]) -> SIMD[DType.int8, 4 * n]:
    return x


def next[n: Int](x: SIMD[DType.int8, bump[1](n)]) -> SIMD[DType.int8, n + 1]:
    return x


def method[n: Int](x: SIMD[DType.int8, Widths.twice(n)]) -> SIMD[DType.int8, 2 * n]:
    return x


def storage[n: Int](x: SIMD[DType.int8, doubled(n)]) -> SIMD[DType.int8, n + n]:
    return x


def picked[n: Int](x: SIMD[DType.int8, pick(n)]) -> SIMD[DType.int8, pick(n)]:
    return x


def bounded[n: Int]() where twice(n) > 2:
    print(n)


def local[n: Int]() -> Int:
    comptime m = twice(n)
    return m


def main():
    print(same[2](SIMD[DType.int8, 4](1)))
    print(four[1](SIMD[DType.int8, 4](2)))
    print(next[3](SIMD[DType.int8, 4](3)))
    print(method[2](SIMD[DType.int8, 4](4)))
    print(storage[1](SIMD[DType.int8, 2](5)))
    print(picked[2](SIMD[DType.int8, 2](6)))
    var v: SIMD[DType.int8, 4] = SIMD[DType.int8, twice(2)](7)
    print(v)
    print(Lanes[2]().lanes)
    print(local[4]())
    bounded[5]()
    var a = 5
    print(twice(a), quad(a), bump[2](a), pick(a), doubled(a))
