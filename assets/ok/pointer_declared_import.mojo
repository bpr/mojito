# The bundled `std/memory/pointer.mojo` declares upstream's `struct
# Pointer`, exported from `std.memory` beside its aliases. An application
# of the declared struct is the compiler's pointer type, so an alias, a
# bare `Pointer[T, o]` annotation, and `Pointer(to=)` name one type, and an
# alias's defaulted `address_space` keeps a placeholder parameter's
# `ImmPointer[T, _]` free of any binder but its origin.
from std.memory import (
    ImmOpaquePointer,
    ImmPointer,
    MutOpaquePointer,
    MutPointer,
    OpaquePointer,
    Pointer,
)


def show(p: ImmPointer[Int, _]):
    print(p[])


def main():
    var x = 5
    var p: MutPointer[Int, origin_of(x)] = Pointer(to=x)
    print(p[])
    var q: Pointer[Int, origin_of(x)] = Pointer(to=x)
    q[] = 7
    print(q[])
    show(Pointer(to=x).unsafe_mut_cast[False]())
    print(Int(Pointer(to=x)) != 0)
