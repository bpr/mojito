# A `ref[...] self` clause is the receiver's `ref` contract, as on any
# `ref` parameter: a call deduces its binders from the receiver's place
# (its address space and origin), through a plain call, the explicit
# `S.m(s)` form, and an operator, and a `ref` result over a binder the
# clause names borrows the receiver.


struct S:
    var x: Int

    def __init__(out self):
        self.x = 1

    def space[a: AddressSpace](ref[_, a] self) -> Int:
        return Int(a._value)

    def sp[
        origin: Origin, address_space: AddressSpace, //
    ](ref[origin, address_space] self) -> Int:
        return Int(address_space._value)

    def gen(ref[AddressSpace.GENERIC] self) -> Int:
        return self.x

    def __add__[a: AddressSpace](ref[_, a] self, o: Int) -> Int:
        return Int(a._value) + o

    def first[o: Origin](ref[o] self) -> ref[o] Self:
        return self


def main():
    var s = S()
    var p = Pointer(to=s)
    var q = p.unsafe_address_space_cast[AddressSpace.GLOBAL]()
    print(s.space())
    print(q[].space())
    print(s.sp(), q[].sp(), S().sp(), s.gen())
    print(S.space(s))
    print(s + 2)
    print(q[].space[AddressSpace.GLOBAL]())
    s.first().x = 5
    print(s.x)
