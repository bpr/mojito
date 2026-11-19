# A `_` in a method call's bracket leaves an origin binder to the call's one
# solve, which binds it from the argument's place beside the value binders
# the bracket binds, on an instance, a static method, and a generic struct.


struct S:
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def get[o: ImmOrigin, n: Int](self, p: Pointer[Int, o]) -> Int:
        return p[] + self.k + n

    @staticmethod
    def st[o: ImmOrigin, n: Int](p: Pointer[Int, o]) -> Int:
        return p[] - n


struct G[T: ImplicitlyCopyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v

    def view[o: ImmOrigin](self, p: Pointer[Int, o]) -> Pointer[Int, o]:
        return p

    def pick[o: ImmOrigin, n: Int](self, p: Pointer[Int, o]) -> Int:
        return p[] + n


def main():
    var a = 10
    var s = S(3)
    print(s.get[_, 5](Pointer(to=a)))
    print(s.get[o=_, n=2](Pointer(to=a)))
    print(S.st[_, 4](Pointer(to=a)))
    var g = G[Int](1)
    print(g.view[origin_of(a)](Pointer(to=a))[])
    print(g.view(Pointer(to=a))[])
    print(g.pick[_, 7](Pointer(to=a)))
    print(g.pick[n=1](Pointer(to=a)))
