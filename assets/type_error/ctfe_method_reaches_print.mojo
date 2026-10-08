# expect: is not safe for VM-backed compile-time execution
# A compile-time evaluation that reaches `print` through a method call is
# rejected, as one through a direct call is: the pin prints at compile time,
# which Mojito does not (docs/non-goals.md).
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
