# A struct's origin-mutability binder reads as a value, as upstream's does:
# `Self.mut`, a `comptime` alias of it, and `Self.origin.mut` all read the
# mutability its `Origin[mut=mut]` origin argument decides, one member
# instance per closed mutability, and `Q[o].mut` under an enclosing
# `o: Origin[mut=m]` reads `m`.


struct Q[mut: Bool, //, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def g(self) -> Bool:
        return Self.mut

    def h(self) -> Bool:
        comptime m = Self.mut
        return m

    def k(self) -> Bool:
        return Self.origin.mut


def show[o: ImmOrigin]() -> Bool:
    return Q[o](1).g()


def generic[m: Bool, //, o: Origin[mut=m]]() -> Bool:
    return Q[o].mut


def main():
    var x = 1
    print(Q[origin_of(x)](4).g())  # True
    print(show[origin_of(x)]())  # False
    print(Q[ImmutAnyOrigin](4).g())  # False
    print(Q[MutAnyOrigin](4).g())  # True
    print(Q[origin_of(x)](4).h())  # True
    print(Q[ImmutAnyOrigin](4).h())  # False
    print(Q[origin_of(x)](4).k())  # True
    print(Q[ImmutAnyOrigin](4).k())  # False
    print(Q[origin_of(x)].mut)  # True
    print(generic[origin_of(x)]())  # True
    print(generic[ImmutAnyOrigin]())  # False
