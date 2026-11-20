# Upstream's `Pointer` aliases are `comptime` declarations of
# `std/memory/pointer.mojo`, exported from `std.memory` and the prelude: an
# alias binds its origin argument, a parameter's `_`, and its keyword
# `address_space=` wherever its body names them. A struct header default
# reads a `comptime` member of a struct declared after it.
from std.memory import ImmOpaquePointer, MutOpaquePointer


comptime MP[T: AnyType, origin: MutOrigin] = Pointer[T, origin]


struct Q[n: K = .ONE]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n.v


struct K(ImplicitlyCopyable, TrivialRegisterPassable):
    var v: Int
    comptime ONE = K(1)

    def __init__(out self, v: Int):
        self.v = v


def write(p: MutPointer[Int, _]):
    p[] = 7


def read(p: ImmPointer[Int, _]) -> Int:
    return p[]


def address(p: OpaquePointer[_]) -> Int:
    return Int(p)


def mutable_address(p: MutOpaquePointer[_]) -> Int:
    return Int(p)


def kind(p: ImmOpaquePointer[_]) -> Int:
    return 1


def kind(p: ImmPointer[Int, _]) -> Int:
    return 2


def bump(p: MP[Int, _]) -> Int:
    p[] += 1
    return p[]


def main():
    var x = 5
    var p: MutPointer[
        Int, origin_of(x), address_space=AddressSpace.GENERIC
    ] = Pointer(to=x)
    write(p)
    print(read(p))
    print(bump(p))
    var q: MP[Int, origin_of(x)] = Pointer(to=x)
    print(q[])
    var n: NoneType = None
    var o: OpaquePointer[origin_of(n)] = Pointer(to=n)
    print(address(o) == mutable_address(o))
    print(kind(Pointer(to=x)), kind(o))
    var none: OptionalPointer[Int, origin_of(x)] = None
    print(Bool(none))
    print(Q().get())
