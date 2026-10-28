# expect: cannot infer type parameter 'k'
# A local `comptime L = W(n)` over a builtin-folded `W.__init__` holds the
# folded struct value under the alias, not the construction: `B[W(n)]`
# solves `g`'s `k` from `W(k)`, but `B[L]` does not, as at the pin.
struct W(TrivialRegisterPassable):
    var v: Int

    @always_inline("builtin")
    def __init__(out self, value: Int):
        self.v = value


struct B[w: W]:
    def __init__(out self):
        pass


def g[k: Int](x: B[W(k)]):
    print(k)


def f[n: Int]():
    g(B[W(n)]())
    comptime L = W(n)
    g(B[L]())


def main():
    f[2]()
