# PROBE: a keyword call through a trait bound to a renamed witness.
#
# The pin binds a keyword through a trait bound by the requirement's names
# and passes the values to the witness by position, so `u.f(x=1, z=2)`
# reaches `S.f(self, z: Int, x: Int)` as `z=1, x=2` and prints 312. Mojito
# rejects the keyword call (roadmap R420).
trait T:
    def f(self, x: Int, z: Int) -> Int:
        ...


struct S(T):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def f(self, z: Int, x: Int) -> Int:
        return self.n * 100 + z * 10 + x


def g[U: T](u: U) -> Int:
    return u.f(x=1, z=2)


def main():
    print(g(S(3)))
