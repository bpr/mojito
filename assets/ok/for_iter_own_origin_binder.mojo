# A loop's implicit `__iter__` call binds the callee's own compile-time
# parameters from the iterable, as an ordinary `c.__iter__()` call does: the
# receiver's `ref[o] self` binds `o` and its `mut=` binder `m`, so the
# iterator reads `True` over a `var` and `False` from a read-only caller or
# a temporary, in a `for` statement and a comprehension alike. A `__next__`
# with its own defaulted parameter carries it too.
struct It[mut: Bool, //, o: Origin[mut=mut]]:
    var i: Int
    var m: Bool

    def __init__(out self, m: Bool):
        self.i = 0
        self.m = m

    def __next__[step: Int = 1](mut self) raises StopIteration -> Bool:
        if self.i >= 2:
            raise StopIteration()
        self.i += step
        return self.m


struct C:
    var x: Int

    def __init__(out self):
        self.x = 1

    def __iter__[m: Bool, //, o: Origin[mut=m]](ref[o] self) -> It[o]:
        return It[o](m)


def make() -> C:
    return C()


def read_only(c: C):
    print([b for b in c])


def main():
    var c = C()
    print([b for b in c])
    read_only(c)
    for b in make():
        print(b)
    var d = C()
    for b in d:
        print(b)
