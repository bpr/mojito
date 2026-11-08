# A `SIMD` length need not be a power of two: the pin admits
# `SIMD[DType.float32, 3]` as a type and constructs one, since the
# `comptime assert` of the `@always_inline("builtin")`
# `_simd_construction_checks` folds away with the call. A call of a builtin
# procedure whose arguments are all compile-time folds the same way, its
# assert never judged.


@always_inline("builtin")
def only_two[n: Int]():
    comptime assert n == 2, "n must be 2"


struct Width[n: Int]:
    @always_inline("builtin")
    @staticmethod
    def check():
        comptime assert Self.n == 2, "n must be 2"


def takes_three(x: SIMD[DType.float32, 3]) -> Int:
    return 1


def main():
    print(takes_three(SIMD[DType.float32, 3](1.0)))
    var lanes = SIMD[DType.float32, 3](1.0)
    print(lanes[1])
    var splat = SIMD[DType.int32, 3](7)
    print(splat[2])
    only_two[3]()
    Width[5].check()
    print("folded")
