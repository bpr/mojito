# expect: SIMD element
# Upstream's `Intable` constructor builds one scalar; an `Intable` struct is
# no element of the `*elems: Scalar[Self.dtype]` overload.
@fieldwise_init
struct Meters(Intable, ImplicitlyCopyable):
    var v: Int

    def __int__(self) -> Int:
        return self.v


def main():
    print(SIMD[DType.uint8, 2](Meters(1), Meters(2)))
