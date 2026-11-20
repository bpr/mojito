# An initializer's own origin binder is bound in the construction's one
# solve with its type and value binders, as upstream's call binds every
# declared parameter: on a generic struct, an overloaded one, and with the
# binder's `mut=` binder, as on a struct without parameters.


struct W[T: Copyable & Deinitable]:
    var v: Self.T
    var q: Int

    def __init__[o: ImmOrigin](out self, x: Self.T, p: Pointer[Int, o]):
        self.v = x.copy()
        self.q = p[]


struct M[T: Copyable & Deinitable]:
    var v: Self.T
    var q: Int

    def __init__[
        mut: Bool, //, o: Origin[mut=mut]
    ](out self, x: Self.T, p: Pointer[Int, o]):
        self.v = x.copy()
        self.q = p[] + 1

    def __init__(out self, x: Self.T, y: Self.T, z: Self.T):
        self.v = x.copy()
        self.q = 0


struct V:
    var q: Int

    def __init__[T: Copyable, o: ImmOrigin](out self, x: T, p: Pointer[Int, o]):
        self.q = p[] + 2


def main():
    var k = 5
    var w = W(1, Pointer(to=k))
    print(w.q)
    var m = M(1, Pointer(to=k))
    print(m.q)
    var m2 = M(1, 2, 3)
    print(m2.q)
    var v = V(1, Pointer(to=k))
    print(v.q)
