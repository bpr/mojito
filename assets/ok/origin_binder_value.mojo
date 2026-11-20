# An origin binder is a value parameter of its `Origin` type, as upstream's
# is: `o: Origin[mut=m]` enters its signature and body as a value of type
# `Origin[mut=m, _mlir_origin=o]`, so `o.mut`, `type_of(o)`,
# `ImmOrigin(o)`, `o.unsafe_mut_cast[m]()`, a run-time copy, a `where`
# clause, a nested function, and a struct's `Self.o` all read it.


def h(v: ImmOrigin) -> Bool:
    return v.mut


def k[o: Origin[mut=False]]() -> Bool:
    comptime T = type_of(o)
    return T.mut


def reads[o: MutOrigin](ref[o] x: Int):
    print(o.mut)  # True
    comptime T = type_of(o)
    print(T.mut)  # True
    comptime c = ImmOrigin(o)
    print(c.mut)  # False
    var v = o
    print(h(v))  # False


def cast[o: MutOrigin]():
    comptime c = o.unsafe_mut_cast[False]()
    print(c.mut)  # False


def only_mutable[o: MutOrigin](ref[o] x: Int) where o.mut:
    print(x)  # 1


def nested[o: MutOrigin](ref[o] x: Int):
    def g() -> Bool:
        return o.mut

    print(g())  # True


def through_pointer[m: Bool, //, o: Origin[mut=m]](p: Pointer[Int, o]):
    print(o.mut)  # True


struct S[o: ImmOrigin]:
    def __init__(out self):
        pass

    def get(self) -> Bool:
        return Self.o.mut


def main():
    print(k[ImmStaticOrigin]())  # False
    var x = 1
    reads(x)
    cast[MutUntrackedOrigin]()
    only_mutable(x)
    nested(x)
    through_pointer(Pointer(to=x))
    var s = S[ImmStaticOrigin]()
    print(s.get())  # False
