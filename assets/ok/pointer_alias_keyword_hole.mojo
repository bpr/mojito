# A keyword `_` (`address_space=_`) or a `...` on an applied parametric
# alias leaves that parameter unbound, as on the struct it aliases: each
# becomes an infer-only parameter of the `def`, for std's `ImmPointer` and
# `MutPointer` and for a module's own alias of the same shape. A
# `MutPointer`'s per-call origin keeps the alias's `MutOrigin`, so the
# pointee is writable through it, and `...` leaves the keyword-only
# `address_space` unbound too.
from std.memory.address_space import AddressSpace

comptime MyImm[
    T: AnyType, origin: ImmOrigin, *, address_space: AddressSpace = .GENERIC
] = Pointer[T, origin, address_space=address_space]


def imm(ptr: ImmPointer[Int, _, address_space=_]) -> Int:
    return ptr[]


def direct(ptr: Pointer[Int, _, address_space=_]) -> Int:
    return ptr[]


def own(ptr: MyImm[Int, _, address_space=_]) -> Int:
    return ptr[]


def bump_hole(ptr: MutPointer[Int, _, address_space=_]):
    ptr[] += 1


def bump_rest(ptr: MutPointer[Int, ...]):
    ptr[] += 10


def forward(ptr: MutPointer[Int, _, address_space=_]):
    bump_rest(ptr)


def main():
    var x = 4
    print(direct(Pointer(to=x)))
    print(imm(Pointer(to=x)))
    print(own(Pointer(to=x)))
    bump_hole(Pointer(to=x))
    bump_rest(Pointer(to=x))
    forward(Pointer(to=x))
    print(x)
