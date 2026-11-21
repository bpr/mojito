# A `Pointer` in a non-generic address space, upstream's `ptr addrspace(N)`
# on the host: `unsafe_address_space_cast` to `AddressSpace.SHARED` (an
# `addrspacecast`) and back, a binder deducing the space, a struct field in
# the space read in a callee, an aggregate copied out of the space (a
# `memcpy` from `addrspace(3)`), a field written through it, a `ref`
# binding over its pointee, a register-passable value passed from it by
# value, and the target's per-space pointer width (x86-64's `p270` is
# 32-bit).
# Upstream prints `5 3`, `5`, `7 7`, `1 2 4 5 4`, `20 3`, `4`, and `8`.
from std.memory import AddressSpace
from std.sys import size_of


def space_of[a: AddressSpace](p: Pointer[Int, _, address_space=a]) -> Int:
    return Int(a)


struct Holder[o: Origin]:
    var p: Pointer[Int, Self.o, address_space=AddressSpace.SHARED]

    def __init__(
        out self, p: Pointer[Int, Self.o, address_space=AddressSpace.SHARED]
    ):
        self.p = p


def read[o: Origin](h: Holder[o]) -> Int:
    return h.p[]


@fieldwise_init
struct Pair(ImplicitlyCopyable):
    var a: Int
    var b: Int


@fieldwise_init
struct Cell(TrivialRegisterPassable):
    var a: Int
    var b: Int


@no_inline
def total(x: Cell) -> Int:
    return x.a + x.b


@no_inline
def tenfold(x: Int) -> Int:
    return x * 10


def main():
    var x = 5
    var q = Pointer(to=x).unsafe_address_space_cast[AddressSpace.SHARED]()
    print(q[], space_of(q))
    print(q.unsafe_address_space_cast[AddressSpace.GENERIC]()[])

    var y = 7
    var h = Holder(
        Pointer(to=y).unsafe_address_space_cast[AddressSpace.SHARED]()
    )
    print(read(h), y)

    var s = Pair(1, 2)
    var ps = Pointer(to=s).unsafe_address_space_cast[AddressSpace.SHARED]()
    var t = ps[]
    ps[].b = 5
    ref r = ps[]
    r.a = 4
    print(t.a, t.b, s.a, s.b, r.a)

    var c = Cell(1, 2)
    var pc = Pointer(to=c).unsafe_address_space_cast[AddressSpace.SHARED]()
    print(tenfold(pc[].b), total(pc[]))

    print(
        size_of[Pointer[Int, MutAnyOrigin, address_space=AddressSpace(270)]]()
    )
    print(size_of[Pointer[Int, MutAnyOrigin, address_space=AddressSpace(3)]]())
