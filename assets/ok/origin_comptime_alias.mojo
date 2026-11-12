# An alias of an origin type is another spelling of it: a module or function
# `comptime` alias of `Origin[mut=True]` or of `MutOrigin`, and a chain of
# them, bound a binder as the type itself does.

comptime MutO = Origin[mut=True]
comptime AlsoMut = MutO
comptime M = MutOrigin
comptime Imm = ImmOrigin


def bump[o: MutO](ref[o] x: Int):
    x += 1


def bump_twice[o: AlsoMut](ref[o] x: Int):
    x += 2


def bump_m[o: M](ref[o] x: Int):
    x += 10


def read[o: Imm](p: Pointer[Int, o]) -> Int:
    return p[]


struct Holder[o: AlsoMut]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p

    def get(self) -> Int:
        return self.p[]

    def add[q: M](self, ref[q] x: Int):
        x += self.p[]


def main():
    comptime Local = Origin[mut=True]
    comptime LocalM = MutOrigin
    var n = 1
    bump(n)
    print(n)
    bump_twice(n)
    print(n)
    bump_m(n)
    print(n)
    print(read(Pointer(to=n)))
    var k = 5
    var h = Holder(Pointer(to=k))
    print(h.get())
    var total = 100
    h.add(total)
    print(total)
