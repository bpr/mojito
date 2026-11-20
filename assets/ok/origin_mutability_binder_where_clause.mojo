# A `where` clause over an origin's `mut=` binder is a clause of the
# function's whole declared list, checked with every parameter the call
# binds: an overload it rejects leaves the others, a `ref` argument binds
# the binder as a pointer's origin does, a forwarding body assumes its own
# clause, and a method, a static method, and an initializer check theirs
# alike, as at the pin.
def w[mut: Bool, //, o: Origin[mut=mut]](x: Pointer[Int, o]) -> Int where mut:
    return 7


def w(x: Int) -> Int:
    return 1


def through_ref[
    mut: Bool, //, o: Origin[mut=mut]
](ref [o] x: Int) -> Int where mut:
    return x


def forward[
    mut: Bool, //, o: Origin[mut=mut]
](x: Pointer[Int, o]) -> Int where mut:
    return w(x)


struct S:
    var v: Int

    def __init__[
        mut: Bool, //, o: Origin[mut=mut]
    ](out self, x: Pointer[Int, o]) where mut:
        self.v = x[]

    def m[
        mut: Bool, //, o: Origin[mut=mut]
    ](self, x: Pointer[Int, o]) -> Int where mut:
        return self.v + 1

    def m(self, x: Int) -> Int:
        return x

    @staticmethod
    def st[mut: Bool, //, o: Origin[mut=mut]](x: Pointer[Int, o]) -> Int where mut:
        return x[] * 2


def main():
    var a = 5
    print(w(Pointer(to=a)), w(2))
    print(through_ref(a))
    print(forward(Pointer(to=a)))
    var s = S(Pointer(to=a))
    print(s.m(Pointer(to=a)), s.m(4))
    print(S.st(Pointer(to=a)))
