# A method call's bracket binds an explicit origin argument in its origin
# binder's declared slot, positionally or by keyword, beside the method's
# other parameters, on an instance, a static method, and a generic struct.


struct S:
    var k: Int

    def __init__(out self, k: Int):
        self.k = k

    def get[o: ImmOrigin, n: Int](self, p: Pointer[Int, o]) -> Int:
        return p[] + self.k + n

    def view[o: ImmOrigin](self, p: Pointer[Int, o]) -> Pointer[Int, o]:
        return p

    def __getitem__[o: ImmOrigin, n: Int](self, p: Pointer[Int, o], q: Int) -> Int:
        return p[] * self.k + q + n

    @staticmethod
    def st[o: ImmOrigin](p: Pointer[Int, o]) -> Int:
        return p[] - 1


struct G[T: ImplicitlyCopyable & Deinitable]:
    var v: Self.T

    def __init__(out self, v: Self.T):
        self.v = v

    def pick[
        o: ImmOrigin, U: ImplicitlyCopyable & Deinitable
    ](self, p: Pointer[U, o]) -> U:
        return p[]


def main():
    var a = 10
    var s = S(3)
    print(s.get[origin_of(a), 5](Pointer(to=a)))
    print(s.get[o=origin_of(a), n=2](Pointer(to=a)))
    print(s.view[origin_of(a)](Pointer(to=a))[])
    print(s.__getitem__[origin_of(a), 4](Pointer(to=a), 1))
    print(s.st[origin_of(a)](Pointer(to=a)))
    print(S.st[origin_of(a)](Pointer(to=a)))
    var g = G[Int](1)
    print(g.pick[origin_of(a), Int](Pointer(to=a)))
    print(g.pick[origin_of(a)](Pointer(to=a)))
