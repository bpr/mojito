# `Origin` declares upstream's `@implicit __init__(v: Origin) ->
# ImmOrigin[_mlir_origin=v._mlir_origin]`, so every `Origin` type converts
# an origin to an immutable one through it: `ImmOrigin(o)`,
# `Origin[mut=False](o)`, a user alias, and the bare `Origin`, in a struct's
# origin slot, a `Pointer`'s origin, and a `ref` clause, and an argument
# converts implicitly where an `ImmOrigin` is expected.


struct S[o: Origin]:
    def __init__(out self):
        pass


comptime MyImm = Origin[mut=False]


def read[o: MutOrigin](ref[o] x: Int) -> ref[ImmOrigin(o)] Int:
    return x


def is_mut(v: ImmOrigin) -> Bool:
    return v.mut


def any_origin(v: Origin) -> Int:
    return 1


def f[o: MutOrigin](ref[o] x: Int):
    var a: S[MyImm(o)] = S[ImmOrigin(o)]()
    var b: S[ImmOrigin(ImmOrigin(o))] = S[ImmOrigin(o)]()
    var c: S[ImmOrigin(ImmStaticOrigin)] = S[ImmStaticOrigin]()
    var d: S[ImmOrigin(MutUntrackedOrigin)] = S[ImmUntrackedOrigin]()
    var e: S[Origin(o)] = S[Origin[mut=False](o)]()


def main():
    var x = 7
    f(x)
    var p: Pointer[Int, ImmOrigin(origin_of(x))] = Pointer(to=x)
    print(p[])
    print(read(x))
    print(is_mut(MutUntrackedOrigin))
    print(any_origin(ImmStaticOrigin))
    comptime O = ImmOrigin[_mlir_origin=origin_of(x)._mlir_origin]
    print(O.mut)
