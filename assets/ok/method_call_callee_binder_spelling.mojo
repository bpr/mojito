# A signature's call to a method closes the callee's own binders, never a
# caller binder of the same spelling: `radd`'s `rhs.add(self)` binds
# `add`'s `rhs` to the caller's `self`, while the receiver is the caller's
# own `rhs`, so the result is `W[2 * 10 + 1]`.
@fieldwise_init
struct W[n: Int](Copyable):
    def add(self, rhs: W[_]) -> W[Self.n * 10 + rhs.n]:
        return {}

    def radd(self, rhs: W[_]) -> type_of(rhs.add(self)):
        return rhs.add(self)

    def add_named[m: Int](self, rhs: W[m]) -> W[Self.n * 10 + m]:
        return {}

    def radd_named[m: Int](self, rhs: W[m]) -> type_of(rhs.add_named(self)):
        return rhs.add_named(self)

    def g(self, a: W[_], b: W[Self.n]) -> W[Self.n * 10 + a.n]:
        return {}

    def h(self, a: W[_]) -> type_of(a.g(self, W[a.n]())):
        return a.g(self, W[a.n]())


def radd(lhs: W[_], rhs: W[_]) -> type_of(rhs.add(lhs)):
    return rhs.add(lhs)


def main():
    print(W[1]().radd(W[2]()).n)
    print(W[1]().radd_named(W[2]()).n)
    print(radd(W[1](), W[2]()).n)
    print(W[1]().h(W[2]()).n)
