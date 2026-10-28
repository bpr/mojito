# A `TrivialRegisterPassable` value passed as a read argument beside its own
# `mut` borrow is an independent copy, as the pin passes it in a register:
# `x *= x` and `bump(x, x)` read the value before the call mutates it.


struct P(ImplicitlyCopyable, TrivialRegisterPassable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __imul__(mut self, rhs: Self):
        self.v = self.v * rhs.v
        self.v += rhs.v


def bump(mut a: P, b: P):
    a.v += 1
    a.v += b.v


def main():
    var x = P(3)
    x *= x
    print(x.v)
    bump(x, x)
    print(x.v)
