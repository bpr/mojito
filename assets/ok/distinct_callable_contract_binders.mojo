# Each callable contract's `def[...]` binders belong to that contract alone:
# a contract declared earlier with a differently spelled binder (`wq`) does
# not lend its spelling to a later one (`k`), whether the two bound
# parameters of module functions, of struct methods (on a plain and on a
# parameterized struct), or are the bodies of comptime aliases.
comptime First = def[wq: Int](SIMD[DType.int32, wq]) -> SIMD[DType.int32, wq]
comptime Second = def[k: Int](SIMD[DType.int32, k]) capturing -> None


def probe[
    func: def[wq: Int](SIMD[DType.int32, wq]) -> SIMD[DType.int32, wq]
]() -> Int:
    return 0


def run[h: def[k: Int](SIMD[DType.int32, k]) capturing -> None]():
    h[4](SIMD[DType.int32, 4](1))


def run_alias[h: Second]():
    h[2](SIMD[DType.int32, 2](3))


struct A:
    def __init__(out self):
        pass

    def probe[
        func: def[wq: Int](SIMD[DType.int32, wq]) -> SIMD[DType.int32, wq]
    ](self) -> Int:
        return 0


struct B:
    def __init__(out self):
        pass

    def run[h: def[k: Int](SIMD[DType.int32, k]) capturing -> None](self):
        h[4](SIMD[DType.int32, 4](2))


struct C[dtype: DType]:
    def __init__(out self):
        pass

    def probe[
        func: def[wq: Int](SIMD[Self.dtype, wq]) -> SIMD[Self.dtype, wq]
    ](self) -> Int:
        return 0

    def run[h: def[k: Int](SIMD[DType.int32, k]) capturing -> None](self):
        h[2](SIMD[DType.int32, 2](5))


def show[n: Int](v: SIMD[DType.int32, n]) -> None:
    print(v)


def main():
    run[show]()
    run_alias[show]()
    B().run[show]()
    C[DType.int32]().run[show]()
