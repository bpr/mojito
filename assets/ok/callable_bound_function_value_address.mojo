# A function value at a `def(...)`-bound `read` parameter is passed by the
# address of its storage, as at Mojo, where the bound does not refine
# `RegisterPassable`: a function name, a nested `def`, a forwarded bound
# value, both kinds of lambda, and `Tuple()`'s `init_with` lambda each still
# reach their callee.
def dbl(x: Int) -> Int:
    return x * 2


def apply[F: def(Int) -> Int](f: F) -> Int:
    return f(3)


def twice[F: def(Int) -> Int](f: F) -> Int:
    return apply(f) + apply(f)


struct D(Defaultable, Movable):
    var v: Int

    def __init__(out self):
        self.v = 7


def main():
    print(apply(dbl))

    def g(x: Int) -> Int:
        return x * 3

    print(apply(g))
    print(twice(dbl))
    var k = 5
    print(apply(lambda (x: Int) {k} -> Int: x + k))
    var s = String("ab")
    print(apply(lambda (x: Int) {var s^} -> Int: x + s.byte_length()))
    var t = Tuple[D, D]()
    print(t[0].v, t[1].v)
