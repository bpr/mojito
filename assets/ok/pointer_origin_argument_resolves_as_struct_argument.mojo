# A `Pointer`'s origin argument resolves as any struct's does: the
# receiver's `origin_of(self)` names one origin in a method's signature and
# body, and a pointer parameter's `other.origin` reads its bound origin.


struct View[mut: Bool, //, o: Origin[mut=mut]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


struct Box:
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def view(ref self) -> View[origin_of(self)]:
        return View[origin_of(self)](
            Pointer(to=self.x).unsafe_origin_cast[origin_of(self)]()
        )


def imm[
    mut: Bool, //, o: Origin[mut=mut]
](other: Pointer[Int, o]) -> Pointer[Int, ImmOrigin(other.origin)]:
    return other.as_imm()


def main():
    var b = Box(1)
    print(b.view().p[])
    var x = 5
    var y = 6
    print(imm(Pointer(to=x))[])
    print(imm(Pointer(to=y))[])
