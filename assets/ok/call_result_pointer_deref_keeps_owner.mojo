# A pointer returned by a call keeps the owner its origin names alive until
# the dereference reads it, even when the owner has no later use. The result
# is bound to a hidden slot carrying the loan a `var q = idp(p)` binding
# would establish.
# The same holds for a generic identity, a nested call, a method result, the
# keyword `unsafe_offset` spelling, a field read through the deref, and an
# owner whose destructor runs only after the read.
def idp[o: MutOrigin](v: Pointer[Int, o]) -> Pointer[Int, o]:
    return v


def id[T: ImplicitlyCopyable](v: T) -> T:
    return v


@fieldwise_init
struct Holder[o: MutOrigin](Copyable):
    var p: Pointer[Int, Self.o]

    def get(self) -> Pointer[Int, Self.o]:
        return self.p


struct Noisy:
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __deinit__(deinit self):
        print("del", self.v)


def idn[o: MutOrigin](v: Pointer[Noisy, o]) -> Pointer[Noisy, o]:
    return v


def main():
    var x = 7
    var p = Pointer(to=x)
    print(idp(p)[])
    var g = 8
    var gp = Pointer(to=g)
    print(id(gp)[])
    var n = 9
    var np = Pointer(to=n)
    print(idp(idp(np))[])
    var h = 10
    var holder = Holder(Pointer(to=h))
    print(holder.get()[])
    var k = 11
    var kp = Pointer(to=k)
    print(idp(kp)[unsafe_offset=0])
    var w = Noisy(12)
    var wp = Pointer(to=w)
    print(idn(wp)[].v)
    print("end")
