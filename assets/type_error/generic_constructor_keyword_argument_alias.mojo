# expect: 's' is borrowed mutably and also used at the same call
# A generic struct's initializer call binding keyword arguments alias-checks
# them by slot, as at the pin.
@fieldwise_init
struct S(Copyable, Movable):
    var v: Int

struct G[T: AnyType]:
    var a: Int
    def __init__(out self, mut a: S, b: S):
        self.a = a.v + b.v

def main():
    var s = S(1)
    var p = G[Int](b=s, a=s)
    print(p.a)
