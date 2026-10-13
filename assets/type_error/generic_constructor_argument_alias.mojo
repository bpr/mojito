# expect: 's' is borrowed mutably and also used at the same call
# A generic struct's initializer call alias-checks its arguments, as at the
# pin, which names the call 'G[Int]'.
struct S(Copyable, Movable):
    var v: Int
    def __init__(out self, v: Int):
        self.v = v

struct G[T: AnyType]:
    var a: Int
    def __init__(out self, mut a: S, b: S):
        self.a = a.v + b.v

def main():
    var s = S(1)
    var p = G[Int](s, s)
    print(p.a)
