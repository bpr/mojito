# A bare `Pointer` or `Pointer[T, _]` parameter's origin is an infer-only
# binder (with its `mut`) each call binds to its argument's own origin, as
# upstream's automatic parameterization declares it: `p.mut` reads the
# argument's mutability, `other.origin` names the bound origin, and a binder
# origin may be bound to heap storage, so its pointer offsets and indexes.
def is_mut(p: Pointer) -> Bool:
    return p.mut


def as_imm(
    other: Pointer,
) -> Pointer[other.T, ImmOrigin(other.origin), address_space=other.address_space]:
    return rebind[
        Pointer[other.T, ImmOrigin(other.origin), address_space=other.address_space]
    ](other)


def read(p: Pointer[Int, _]) -> Int:
    return p[]


def byte_at[o: Origin](p: Pointer[UInt8, o], n: Int) -> UInt8:
    return p.unsafe_offset(n)[]


def element[o: Origin](p: Pointer[Int, o], i: Int) -> Int:
    return p[unsafe_offset=i]


def main():
    var x = 5
    print(is_mut(Pointer(to=x)))
    print(as_imm(Pointer(to=x))[])
    var a = Pointer(to=x)
    var b = Pointer(to=x)
    print(read(a), read(b))
    var s = String("abc")
    print(byte_at(s.unsafe_ptr(), 1))
    var c: UInt8 = 7
    print(byte_at(Pointer(to=c), 0))
    var xs = [10, 20, 30]
    print(element(xs.unsafe_ptr(), 2))
    print(element(Pointer(to=x), 0))
