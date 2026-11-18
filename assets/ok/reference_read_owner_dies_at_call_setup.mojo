# A register-passable value read through a reference holds its own bits, so
# the owner the reference borrows dies at setup of a call loading the value
# by value, before the callee runs; a call taking the read by address keeps
# the owner past it.

@fieldwise_init
struct H[o: Origin](TrivialRegisterPassable):
    var p: Pointer[Int, Self.o]


@fieldwise_init
struct R(TrivialRegisterPassable):
    var n: Int

    def show(self):
        print("R.show", self.n)


struct V(Movable):
    var x: Int
    var r: R

    def __init__(out self, x: Int):
        self.x = x
        self.r = R(x)

    def __deinit__(deinit self):
        print("del V", self.x)

    def __getitem__(ref self, i: Int) -> ref[self.x] Int:
        return self.x

    def getr(ref self) -> ref[self.r] R:
        return self.r

    def name(self) -> String:
        return String("s") + String(self.x)


struct W(Movable):
    var v: V

    def __init__(out self, var v: V):
        self.v = v^


struct Box[o: Origin](Movable):
    var h: H[Self.o]

    def __init__(out self, h: H[Self.o]):
        self.h = h

    def get(ref self) -> ref[self.h] H[Self.o]:
        return self.h


def mk(x: Int) -> V:
    return V(x)


def show(n: Int):
    print("show", n)


def show2(a: Int, b: Int):
    print("show2", a, b)


def shows(s: String):
    print("shows", s)


def use(h: H):
    print("use", h.p[])


def main():
    show(mk(1)[0])
    show(W(mk(2)).v[0])
    var v3 = mk(3)
    show(v3[0])
    mk(4).getr().show()
    show2(mk(5)[0], mk(6)[0])
    show(mk(7).getr().n)
    var v8 = mk(8)
    ref r8 = v8[0]
    show(r8)
    print(mk(9)[0])
    shows(mk(10).name())
    var v11 = V(11)
    var b = Box(H(Pointer(to=v11.x)))
    use(b.get())
    print("end")
