# A `ref[...] self` clause is the receiver's `ref` contract, as on any
# `ref` parameter: a call deduces its binders from the receiver's place
# (its address space and origin), through a plain call, the explicit
# `S.m(s)` form, an operator, and a subscript's `__getitem__` and
# `__setitem__`, and a `ref` result over a binder the clause names borrows
# the receiver. A subscript over `[m: Bool, //, o: Origin[mut=m]]` reads
# `m` from the receiver's mutability.


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

    def __getitem__[a: AddressSpace](ref[_, a] self, i: Int) -> Int:
        return Int(a._value) * 10 + i

    def __getitem__[a: AddressSpace](ref[_, a] self, sl: Slice) -> Int:
        return Int(a._value) * 100

    def __setitem__[a: AddressSpace](ref[_, a] self, i: Int, v: Int):
        print("set", Int(a._value), i, v)


struct M:
    var x: Int

    def __init__(out self):
        self.x = 0

    def __getitem__[m: Bool, //, o: Origin[mut=m]](ref[o] self, i: Int) -> Bool:
        return m


def read_only(c: M):
    print(c[0])


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
    print(s[5], q[][5], s[1:3], q[][1:3])
    s[2] = 3
    q[][4] = 6
    var c = M()
    print(c[0])
    read_only(c)
