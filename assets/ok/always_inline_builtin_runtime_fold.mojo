# A run-time call of an `@always_inline("builtin")` callable whose every
# argument is a compile-time value folds where it is called, as the pinned
# Mojo's parser folds it, so the body's `comptime assert`, which the fold
# drops, is never judged: `checked[-1](1)` is `2`. The same holds for a
# static method, a builtin calling an asserting one, a call in a generic
# body, and a `SIMD` comparison over compile-time vectors, whose builtin
# `__lt__` asserts one lane.


@always_inline("builtin")
def checked[k: Int](n: Int) -> Int:
    comptime assert k > 0, "positive"
    return n + 1


@always_inline("builtin")
def outer[k: Int](n: Int) -> Int:
    return checked[k](n) * 2


struct G[k: Int]:
    @always_inline("builtin")
    @staticmethod
    def checked(n: Int) -> Int:
        comptime assert Self.k > 0, "positive"
        return n + 1


def generic[k: Int]() -> Int:
    return checked[k](5)


comptime a = SIMD[DType.int32, 4](1, 2, 3, 4)


def folded[v: SIMD[DType.int32, 4]]():
    comptime q = SIMD[DType.int32, 4](from_int=3)
    comptime r = v > q
    print(r)


def main():
    print(checked[-1](1))
    print(G[-1].checked(1))
    print(outer[-1](1))
    print(generic[-3]())
    var x = a < SIMD[DType.int32, 4](2)
    print(x)
    folded[SIMD[DType.int32, 4](1, 2, 3, 4)]()
    var y = 10
    print(checked[2](y))
