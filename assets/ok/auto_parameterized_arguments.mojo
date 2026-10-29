# Each parameter an argument's struct type leaves unbound — the bare struct
# (every parameter, a defaulted one too), `_`, a keyword `_`, or `...` (each
# remaining one) — is an infer-only parameter of the callee, solved per call.
# A module alias over a partially bound struct stands for the struct with its
# hole, a bare variadic struct leaves its pack, and a method's or a nested
# `def`'s argument is parameterized as a module `def`'s is.
@fieldwise_init
struct Pair[A: Copyable & Deinitable, n: Int](Copyable):
    var a: Self.A


@fieldwise_init
struct D[n: Int, m: Int = 5](ImplicitlyCopyable):
    pass


comptime I32 = SIMD[DType.int32, _]


def count(x: List) -> Int:
    return len(x)


def count_any(x: List[_]) -> Int:
    return len(x)


def p(x: Pair) -> Int:
    return type_of(x).n


def q(x: Pair[Int, _]) -> Int:
    return type_of(x).n + x.a


def r(x: Pair[_, 3]) -> Int:
    return type_of(x).n


def s(x: Pair[Int, ...]) -> Int:
    return type_of(x).n


def k(x: Pair[n=_, A=String]) -> Int:
    return type_of(x).n


def d(x: D) -> Int:
    return type_of(x).n * 100 + type_of(x).m


def lanes(x: I32) -> Int:
    return len(x)


def arity(x: Tuple) -> Int:
    return len(x)


struct Counter:
    @staticmethod
    def count(x: List) -> Int:
        return len(x)


def main():
    def inner(x: List) -> Int:
        return len(x)

    print(count([1, 2]), count_any([1, 2, 3]), inner([4]))
    print(p(Pair[String, 7]("a")), q(Pair[Int, 2](5)), r(Pair[Float64, 3](1.0)))
    print(s(Pair[Int, 9](1)), k(Pair[String, 4]("b")))
    print(d(D[1, 2]()), d(D[3]()))
    print(lanes(SIMD[DType.int32, 4](1)), Counter.count(["a", "b", "c"]))
    print(arity((1, "a", 2.5)))
