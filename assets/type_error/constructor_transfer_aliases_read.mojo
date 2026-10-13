# expect: 's' is borrowed mutably and also used at the same call
# A value transferred into an initializer's `var` slot may not also be read
# by the same call, as at the pin.
@fieldwise_init
struct S(Copyable, Movable):
    var v: Int

struct P:
    var a: Int
    def __init__(out self, var a: S, b: S):
        self.a = a.v + b.v

def main():
    var s = S(1)
    var p = P(s^, s)
    print(p.a)
