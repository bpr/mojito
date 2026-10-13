# expect: 's' is borrowed mutably and also used at the same call
# A construction is its `__init__`'s call: a `mut` argument may not alias a
# read argument of the same initializer call, as at the pin.
struct S(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v

struct P:
    var a: Int
    def __init__(out self, mut a: S, b: S):
        self.a = a.v + b.v

def main():
    var s = S(1)
    var p = P(s, s)
    print(p.a)
