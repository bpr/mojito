# `rebind` between two instances of a struct that differ only in an origin
# argument: origins erase from the lowered type, so upstream's
# `as_imm() -> Self.Immutable: return rebind[Self.Immutable](self)` checks.
struct View[mut: Bool, //, o: Origin[mut=mut]](TrivialRegisterPassable):
    comptime Immutable = View[ImmOrigin(Self.o)]
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref [Self.o] x: Int):
        self.p = Pointer(to=x)

    def as_imm(self) -> Self.Immutable:
        return rebind[Self.Immutable](self)

    def get(self) -> Int:
        return self.p[]


def main():
    var x = 7
    var v = View(x)
    var w = v.as_imm()
    var s = String("abcd")
    var halves = StringSpan(s).split_at_grapheme(1)
    print(w.get(), rebind[View[origin_of(x)]](w).get(), halves[0], halves[1])
