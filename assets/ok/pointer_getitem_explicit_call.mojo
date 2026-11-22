# An explicit `p.__getitem__(…)` call runs `Pointer`'s declared getters:
# the zero-argument one, the keyword `unsafe_offset=` one, and the
# deprecated positional one. Reads, stores, `+=`, and field accesses go
# through the reference each returns, over a place pointer and over heap
# pointers. A `ref[Self.o]` result over an `o: Origin[mut=m]` binder takes
# the receiver instance's `mut`, so a read method of a holder whose origin
# is mutable mutates through it, for `Pointer` and a user struct alike.
from std.memory.alloc import unsafe_alloc


struct S(Copyable):
    var f: Int

    def __init__(out self, f: Int):
        self.f = f


struct Acc(Movable):
    var n: Int

    def __init__(out self):
        self.n = 0

    def bump(mut self, k: Int):
        self.n += k


struct Holder[o: Origin[mut=True]]:
    var p: Pointer[Acc, Self.o]

    def __init__(out self, p: Pointer[Acc, Self.o]):
        self.p = p

    def go(self):
        self.p.__getitem__().bump(3)


struct MyPtr[mut: Bool, //, T: AnyType, o: Origin[mut=mut]](
    ImplicitlyCopyable
):
    var p: Pointer[Self.T, Self.o]

    def __init__(out self, p: Pointer[Self.T, Self.o]):
        self.p = p

    def get(self) -> ref[Self.o] Self.T:
        return self.p[]


struct MyHolder[o: Origin[mut=True]]:
    var p: MyPtr[Acc, Self.o]

    def __init__(out self, p: MyPtr[Acc, Self.o]):
        self.p = p

    def go(self):
        self.p.get().bump(4)


def place_pointer():
    var x = 4
    var p = Pointer(to=x)
    print(p.__getitem__())
    p.__getitem__() = 5
    print(x)
    p.__getitem__() += 1
    print(x)
    var s = S(10)
    var q = Pointer(to=s)
    print(q.__getitem__().f)
    q.__getitem__().f = 11
    q.__getitem__().f += 1
    print(s.f)


def heap_pointers():
    var l = [1, 2, 3]
    var p = l.unsafe_ptr()
    p.__getitem__() = 7
    p.__getitem__() += 1
    p.__getitem__(unsafe_offset=1) += 10
    p.__getitem__(2) = 9
    print(l[0], l[1], l[2])
    var h = unsafe_alloc[S](2)
    h.unsafe_write(S(1))
    h.unsafe_offset(1).unsafe_write(S(2))
    print(h.__getitem__().f, h.__getitem__(unsafe_offset=1).f)
    h.__getitem__().f += 5
    h.__getitem__(unsafe_offset=1).f = 9
    print(h.__getitem__().f, h.__getitem__(unsafe_offset=1).f)
    h.unsafe_free()


def holders():
    var a = Acc()
    var h = Holder(Pointer(to=a))
    h.go()
    h.go()
    print(a.n)
    var b = Acc()
    var m = MyHolder(MyPtr(Pointer(to=b)))
    m.go()
    m.go()
    print(b.n)


def main():
    place_pointer()
    heap_pointers()
    holders()
