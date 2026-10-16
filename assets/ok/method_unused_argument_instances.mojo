# A method's runtime argument its body never uses leaves the generator a
# resolved method call names, as upstream's `RemoveUnusedParams` removes
# it first and then the parameters only its type named: `Holder.m[n]`,
# `Holder.mk[n]` (called by position and by keyword), and `PBox[T].m[n]`
# each have one instance for `Arr[1]` and `Arr[2]`, while `Holder.used[n]`,
# which reads `n`, keeps one per `n` but takes no `Arr` either.


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]


struct Holder(Copyable, Movable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    @no_inline
    def m[n: Int](self, a: Arr[n]) -> Int:
        return self.k

    @no_inline
    def mk[n: Int](mut self, x: Int, a: Arr[n], y: Int = 2) -> Int:
        self.k += x
        return self.k + y

    @no_inline
    def used[n: Int](self, a: Arr[n]) -> Int:
        return self.k + n


struct PBox[T: Copyable](Copyable, Movable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    @no_inline
    def m[n: Int](self, a: Arr[n]) -> Int:
        return self.k * 10


def main() raises:
    var h = Holder(3)
    print(h.m(Arr[1](SIMD[DType.int64, 1](1))), h.m(Arr[2](SIMD[DType.int64, 2](1))))
    print(h.mk(1, Arr[1](SIMD[DType.int64, 1](1))), h.mk(y=5, a=Arr[2](SIMD[DType.int64, 2](1)), x=2))
    print(h.used(Arr[1](SIMD[DType.int64, 1](1))), h.used(Arr[2](SIMD[DType.int64, 2](1))))
    var p = PBox[Int](4)
    print(p.m(Arr[1](SIMD[DType.int64, 1](1))), p.m(Arr[2](SIMD[DType.int64, 2](1))))
