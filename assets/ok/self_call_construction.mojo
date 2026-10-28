# `Self(...)` inside a struct's method constructs the enclosing struct over
# its own parameters, by position or keyword, through a written or
# fieldwise `__init__`, from a static method or a nested `def`, as upstream
# binds `Self` to the struct type.
@fieldwise_init
struct P(Copyable, Movable):
    var a: Int
    var b: Bool

    def flipped(self) -> Self:
        return Self(self.a, not self.b)

    def flipped_kw(self) -> Self:
        return Self(b=not self.b, a=self.a)


@fieldwise_init
struct G[T: Copyable & Deinitable & Writable, n: Int](Copyable, Movable):
    var x: Self.T
    var k: Int

    def bumped(self) -> Self:
        return Self(self.x.copy(), self.k + Self.n)

    def bumped_kw(self) -> Self:
        return Self(k=self.k * 2, x=self.x.copy())

    @staticmethod
    def make(var x: Self.T) -> Self:
        return Self(x^, 0)


struct W(Copyable, Defaultable, Movable):
    var v: Int

    def __init__(out self):
        self.v = 42

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, a: Int, b: Int):
        self = Self(a + b)

    def dup(self) -> Self:
        return Self(copy=self)

    def plus(self, d: Int) -> Self:
        return Self(self.v, d)

    def fresh(self) -> Self:
        return Self()

    def nested(self) -> Self:
        def inner(x: Int) -> W:
            return Self(x + 1)

        return inner(self.v)


@fieldwise_init
struct V[o: Origin[mut=False]](Copyable, Movable):
    var p: Pointer[Int, Self.o]

    def again(self) -> Self:
        return Self(self.p)


def main():
    var p = P(1, True)
    print(p.flipped().b, p.flipped_kw().b)
    var g = G[String, 3]("hi", 1)
    var b = g.bumped().bumped_kw()
    print(b.x, b.k)
    var m = G[Int, 5].make(7)
    print(m.x, m.k, m.bumped().k)
    var w = W(1, 2)
    print(w.v, w.dup().v, w.plus(10).v, w.fresh().v, w.nested().v)
    var x = 5
    var v = V(Pointer(to=x))
    print(v.again().p[])
