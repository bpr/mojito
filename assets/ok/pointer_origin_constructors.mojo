# A hand-written constructor whose parameter is a `Pointer` over the
# struct's own origin binder binds that binder from the argument: with an
# explicit `origin_of(x)`, inferred, positionally, and from a method that
# respells its own binder in type position (`V[Self.o]`).
struct V[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref [Self.o] x: Int):
        self.p = Pointer(to=x)

    def __init__(out self, *, unsafe_ptr: Pointer[Int, Self.o]):
        self.p = unsafe_ptr

    def get(self) -> Int:
        return self.p[]

    def twin(self) -> V[Self.o]:
        return V[Self.o](unsafe_ptr=self.p)


struct Plain[o: Origin]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p

    def get(self) -> Int:
        return self.p[]


def main():
    var x = 3
    var a = V[origin_of(x)](unsafe_ptr=Pointer(to=x))
    print(a.get())
    var b = V(unsafe_ptr=Pointer(to=x))
    print(b.get())
    var d = V(x)
    print(d.twin().get())
    var e = Plain[origin_of(x)](Pointer(to=x))
    print(e.get())
