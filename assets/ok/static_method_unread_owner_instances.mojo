# A struct's method keys its instances on the struct parameters it reads,
# as upstream's `RemoveUnusedParams` treats a method generator, whose
# parameters are its struct's followed by its own: `S[1].st()` and
# `S[2].st()` share one instance of a static method that ignores `n`, as do
# a static method and a generic function that only forward `n` to it, while
# a static method that reads `Self.n` keeps one per value, and one of
# `P[a, b]` that reads `a` alone keeps one per `a`.
struct S[n: Int]:
    @staticmethod
    def st() -> Int:
        return 7

    @staticmethod
    def reads() -> Int:
        return Self.n

    @staticmethod
    def chained() -> Int:
        return S[Self.n].st() + 1


struct P[a: Int, b: Int]:
    @staticmethod
    def first() -> Int:
        return Self.a


def caller[n: Int]() -> Int:
    return S[n].st()


def main():
    print(S[1].st(), S[2].st())
    print(S[1].reads(), S[2].reads())
    print(S[1].chained(), S[2].chained())
    print(caller[1](), caller[2]())
    print(P[1, 2].first(), P[1, 3].first(), P[2, 3].first())
