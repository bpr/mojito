# A `Pointer` member no checker rule answers resolves to its declared body:
# `as_imm` on a local, a bare-`ref`, a generic, and a field receiver, its
# result passed to an `ImmPointer` parameter, and a direct `write_to`.


def show(p: ImmPointer[Int, _]):
    print(p[])


def through_ref(ref x: Int) -> Int:
    var p = Pointer(to=x)
    return p.as_imm()[]


def deref[T: Writable & ImplicitlyCopyable, o: Origin](p: Pointer[T, o]) -> T:
    return p.as_imm()[]


struct Holder[o: MutOrigin]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p

    def read(self) -> Int:
        return self.p.as_imm()[]


def main():
    var x = 5
    print(Pointer(to=x).as_imm()[])
    var p = Pointer(to=x)
    show(p.as_imm())
    var q = p.as_imm()
    print(q.as_imm()[])
    print(through_ref(x))
    print(through_ref(7))
    print(deref(Pointer(to=x)))
    var h = Holder(Pointer(to=x))
    print(h.read())
    var s = String()
    p.write_to(s)
    print(s.startswith("0x"))
