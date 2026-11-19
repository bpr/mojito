# Upstream's `Origin.unsafe_mut_cast[m]()`, declared in `std.origin`, builds
# the `#lit.origin.mutcast` of an origin: the same storage at mutability `m`.
# A cast to `False` is the origin `ImmOrigin(o)` is, a cast to the origin's
# own mutability is the origin itself, and nested casts keep the outermost.
# It reads in struct origin slots, `Pointer` origins, `ref` result clauses
# (a `Bool` binder destination included), and module `comptime` values.
struct S[o: Origin]:
    def __init__(out self):
        pass


struct Box[mut: Bool, //, origin: Origin[mut=mut]]:
    var p: Pointer[Int, Self.origin]
    var q: Pointer[Int, Self.origin.unsafe_mut_cast[False]()]

    def __init__(out self, ref[Self.origin] v: Int):
        self.p = Pointer(to=v)
        self.q = Pointer(to=v)

    def get[self_origin: ImmOrigin](ref[self_origin] self) -> ref[
        Self.origin.unsafe_mut_cast[Self.mut]()
    ] Int:
        return self.p[]


def casts[o: MutOrigin, i: ImmOrigin]():
    var a: S[o.unsafe_mut_cast[False]()] = S[o.unsafe_mut_cast[False]()]()
    var b: S[ImmOrigin(o)] = S[o.unsafe_mut_cast[False]()]()
    var c: S[i.unsafe_mut_cast[False]()] = S[i]()
    var d: S[o.unsafe_mut_cast[True]()] = S[o]()
    var e: S[o.unsafe_mut_cast[False]().unsafe_mut_cast[True]()] = S[o]()
    _ = a^
    _ = b^
    _ = c^
    _ = d^
    _ = e^
    print("casts")


def own_binder[m: Bool, //, o: Origin[mut=m]]():
    var a: S[o.unsafe_mut_cast[m]()] = S[o]()
    var b: S[Origin[mut=False](o)] = S[ImmOrigin(o)]()
    _ = a^
    _ = b^
    print("own binder")


def read[o: MutOrigin](ref[o] x: Int) -> ref[o.unsafe_mut_cast[False]()] Int:
    return x


def take[o: MutOrigin](s: S[o.unsafe_mut_cast[False]()]) -> Int:
    return 7


comptime M = ImmStaticOrigin.unsafe_mut_cast[True]()
comptime C[m: Bool] = MutUntrackedOrigin.unsafe_mut_cast[m]()


def main():
    var x = 1
    var y = 2
    casts[origin_of(x), origin_of(y)]()
    own_binder[origin_of(x)]()
    print(read(x))
    var p: Pointer[Int, origin_of(x).unsafe_mut_cast[False]()] = Pointer(to=x)
    print(p[])
    var box = Box(x)
    print(box.get(), box.q[])
    print(take[origin_of(x)](S[origin_of(x).unsafe_mut_cast[False]()]()))
    var u: S[MutUntrackedOrigin.unsafe_mut_cast[False]()] = S[ImmUntrackedOrigin]()
    var s: S[M] = S[ImmStaticOrigin.unsafe_mut_cast[True]()]()
    var g: S[C[False]] = S[ImmUntrackedOrigin]()
    _ = u^
    _ = s^
    _ = g^
    print(M.mut, C[False].mut, C[True].mut)
