# A compile-time callable parameter's contract may name the call's other
# parameters: an earlier bracket argument (`d`), or the receiver struct's
# (`Self.dtype`, directly or through a member alias `Self._T[k]`). The
# argument is matched against the contract with those substituted, for a
# method, a static method, and a free function, and a contract binder that
# shadows the struct's parameter (`def[dtype: DType]`) stays its own.
# Pinned Mojo prints:
# 3
# [3, 4]
# 5
# [6, 7]
# [1, 2]
# [8, 9]
# 1.5
# 3
struct C[dtype: DType]:
    comptime _T[w: Int] = SIMD[Self.dtype, w]

    def __init__(out self):
        pass

    def run[h: def(Scalar[Self.dtype]) capturing -> None](self):
        h(Scalar[Self.dtype](3))

    def run_generic[h: def[k: Int](SIMD[Self.dtype, k]) capturing -> None](
        self,
    ):
        h[2](SIMD[Self.dtype, 2](3, 4))

    @staticmethod
    def run_static[h: def(Scalar[Self.dtype]) capturing -> None]():
        h(Scalar[Self.dtype](5))

    def run_alias[h: def[k: Int](Self._T[k]) capturing -> None](self):
        h[2](SIMD[Self.dtype, 2](6, 7))

    def run_shadow[h: def[dtype: DType](Scalar[dtype]) capturing -> None](
        self,
    ):
        h[DType.float32](Scalar[DType.float32](1.5))
        h[Self.dtype](Scalar[Self.dtype](3))


def run[d: DType, h: def(SIMD[d, 2]) capturing -> None]():
    h(SIMD[d, 2](1, 2))


def run_generic[d: DType, h: def[k: Int](SIMD[d, k]) capturing -> None]():
    h[2](SIMD[d, 2](8, 9))


def show(v: Scalar[DType.int32]):
    print(v)


def show_pair(v: SIMD[DType.int32, 2]):
    print(v)


def show_any[k: Int](v: SIMD[DType.int32, k]):
    print(v)


def show_dtype[dt: DType](v: Scalar[dt]):
    print(v)


def main():
    C[DType.int32]().run[show]()
    C[DType.int32]().run_generic[show_any]()
    C[DType.int32].run_static[show]()
    C[DType.int32]().run_alias[show_any]()
    run[DType.int32, show_pair]()
    run_generic[DType.int32, show_any]()
    C[DType.int32]().run_shadow[show_dtype]()
