# A compile-time evaluation that reaches `print` through a method call
# prints while the program elaborates, as one through a direct call does:
# `side` is the compiler's output, never the executable's.
@fieldwise_init
struct P(Copyable):
    var v: Int

    def get(self) -> Int:
        print("side")
        return self.v


def f(n: Int) -> Int:
    return P(n).get()


def g[k: Int]():
    comptime if f(k) == 1:
        print("one")


def main():
    g[1]()
