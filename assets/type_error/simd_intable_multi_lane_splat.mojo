# expect: SIMD element
# Upstream's `__init__[T: Intable, target_dtype]` returns
# `Scalar[target_dtype]`, so an `Intable` struct never splats across lanes.
@fieldwise_init
struct Meters(Intable, ImplicitlyCopyable):
    var v: Int

    def __int__(self) -> Int:
        return self.v


def main():
    print(SIMD[DType.uint8, 4](Meters(3)))
