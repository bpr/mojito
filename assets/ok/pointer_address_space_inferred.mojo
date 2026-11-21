# An inferred `address_space=_`: a parameter's hole is an infer-only
# binder of the function, which each call solves from its argument (a
# generic pointer and one cast to `AddressSpace.SHARED`), through a plain
# function, a static method, and a homogeneous parameter list; an
# initialized local's hole takes its initializer's space, which a later
# assignment must then match.
# Upstream prints `42 0`, `42 3`, `42`, `42`, and `7 3`.
from std.memory import AddressSpace


def read(p: Pointer[Int, _, address_space=_]) -> Int:
    return p[]


def space_of[a: AddressSpace](p: Pointer[Int, _, address_space=a]) -> Int:
    return Int(a)


def via(p: Pointer[Int, _, address_space=_]) -> Int:
    return space_of(p)


struct Reader:
    @staticmethod
    def read(p: Pointer[Int, _, address_space=_]) -> Int:
        return p[]


def main():
    var x = 42
    var shared = Pointer(to=x).unsafe_address_space_cast[AddressSpace.SHARED]()
    print(read(Pointer(to=x)), via(Pointer(to=x)))
    print(read(shared), via(shared))
    print(Reader.read(shared))
    print(Reader.read(Pointer(to=x)))
    var p: Pointer[Int, origin_of(x), address_space=_] = shared
    p[] = 7
    print(p[], space_of(p))
