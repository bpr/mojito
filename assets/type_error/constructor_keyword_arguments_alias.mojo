# expect: 's' is borrowed mutably and also used at the same call
# Keyword arguments to an initializer alias-check by the slots they bind, as
# at the pin.
@fieldwise_init
struct S(Copyable, Movable):
    var v: Int

struct P:
    var a: Int
    def __init__(out self, mut a: S, b: S):
        self.a = a.v + b.v

def main():
    var s = S(1)
    var p = P(b=s, a=s)
    print(p.a)
