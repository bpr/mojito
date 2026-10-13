# expect: aliasing values passed mutably to 'a' argument and passed immutably to 'rest' argument in 'V' initializer call
# An argument an initializer's `*rest` collector gathers may not alias a
# `mut` argument of the same call, as at the pin.
@fieldwise_init
struct S(Copyable, Movable):
    var v: Int

struct V:
    var n: Int
    def __init__(out self, mut a: S, *rest: S):
        self.n = a.v + len(rest)

def main():
    var s = S(1)
    var v = V(s, s)
    print(v.n)
