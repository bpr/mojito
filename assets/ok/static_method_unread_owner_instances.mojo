# A struct's method keys its instances on the struct parameters it reads,
# as upstream's `RemoveUnusedParams` treats a method generator, whose
# parameters are its struct's followed by its own: `S[1].st()` and
# `S[2].st()` share one instance of a static method that ignores `n`, as do
# a static method and a generic function that only forward `n` to it, while
# a static method that reads `Self.n` keeps one per value, and one of
# `P[a, b]` that reads `a` alone keeps one per `a`. A static method whose
# self-recursive calls spell `S[Self.n]` or `Self` forward `n` to itself, so
# `down` and `down_self` share one instance each, as on the pin.
struct S[n: Int]:
    @staticmethod
    @no_inline
    def st() -> Int:
        return 7

    @staticmethod
    @no_inline
    def reads() -> Int:
        return Self.n

    @staticmethod
    @no_inline
    def chained() -> Int:
        return S[Self.n].st() + 1

    @staticmethod
    @no_inline
    def down(k: Int) -> Int:
        if k == 0:
            return 7
        return S[Self.n].down(k - 1)

    @staticmethod
    @no_inline
    def down_self(k: Int) -> Int:
        if k == 0:
            return 8
        return Self.down_self(k - 1)


struct P[a: Int, b: Int]:
    @staticmethod
    @no_inline
    def first() -> Int:
        return Self.a


@no_inline
def caller[n: Int]() -> Int:
    return S[n].st()


def main():
    print(S[1].st(), S[2].st())
    print(S[1].reads(), S[2].reads())
    print(S[1].chained(), S[2].chained())
    print(caller[1](), caller[2]())
    print(S[1].down(2), S[2].down(3))
    print(S[1].down_self(2), S[2].down_self(3))
    print(P[1, 2].first(), P[1, 3].first(), P[2, 3].first())
