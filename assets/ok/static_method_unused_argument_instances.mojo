# A static method's runtime argument its body never uses leaves the
# generator a call names, as upstream's `RemoveUnusedParams` removes it
# first and then the parameters only its type named: `S.st[n]`, called
# through the struct and through an instance, `S.sk[n]` (called by position
# and by keyword), and `P[T].st[n]`, called through `P[Int]`, `P[Bool]`, and
# an instance, each have one instance for `Arr[1]` and `Arr[2]`, while
# `S.used[n]`, which reads `n`, keeps one per `n` but takes no `Arr` either.


@fieldwise_init
struct Arr[n: Int](Copyable, Movable):
    var v: SIMD[DType.int64, Self.n]


struct S(Copyable, Movable):
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    @staticmethod
    @no_inline
    def st[n: Int](a: Arr[n]) -> Int:
        return 3

    @staticmethod
    @no_inline
    def sk[n: Int](x: Int, a: Arr[n], y: Int = 2) -> Int:
        return x + y

    @staticmethod
    @no_inline
    def used[n: Int](a: Arr[n]) -> Int:
        return n


struct P[T: Copyable](Copyable, Movable):
    def __init__(out self):
        pass

    @staticmethod
    @no_inline
    def st[n: Int](a: Arr[n]) -> Int:
        return 4


def main():
    var s = S(1)
    print(S.st(Arr[1](SIMD[DType.int64, 1](1))), S.st(Arr[2](SIMD[DType.int64, 2](1))))
    print(s.st(Arr[1](SIMD[DType.int64, 1](1))), s.st(Arr[2](SIMD[DType.int64, 2](1))))
    print(S.sk(1, Arr[1](SIMD[DType.int64, 1](1))), S.sk(y=5, a=Arr[2](SIMD[DType.int64, 2](1)), x=2))
    print(S.used(Arr[1](SIMD[DType.int64, 1](1))), S.used(Arr[2](SIMD[DType.int64, 2](1))))
    print(P[Int].st(Arr[1](SIMD[DType.int64, 1](1))), P[Bool].st(Arr[2](SIMD[DType.int64, 2](1))))
    var p = P[Int]()
    print(p.st(Arr[1](SIMD[DType.int64, 1](1))), p.st(Arr[2](SIMD[DType.int64, 2](1))))
