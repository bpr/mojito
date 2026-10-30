# An explicit `T.__init__(...)` call is the construction `T(...)`: a struct
# name, its application, `Self`, or a type parameter selects among the
# type's initializers. The bracket on the type binds the type's parameters;
# the initializer's own are spelled after `.__init__`
# (`D[4].__init__[5]()`), over any struct, `Self`, or a generic caller.


@fieldwise_init
struct P(TrivialRegisterPassable):
    var a: Int


struct Q:
    var a: Int

    def __init__(out self):
        self.a = 3

    def __init__[k: Int](out self):
        self.a = k

    def again(self) -> Self:
        return Self.__init__[8]()


struct Box[n: Int]:
    var a: Int

    def __init__(out self, a: Int):
        self.a = a + Self.n

    def remake(self) -> Self:
        return Self.__init__(self.a)


@fieldwise_init
struct Pair(Copyable):
    var x: Int
    var y: Int

    def again(self) -> Self:
        return Self.__init__(self.x, self.y)


struct D[n: Int = 3]:
    var a: Int

    def __init__[k: Int](out self):
        self.a = k * 10 + Self.n

    def __init__[T: Writable, //, k: Int](out self, x: T):
        self.a = k * 1000 + Self.n

    def again(self) -> Self:
        return Self.__init__[9]()


struct V[n: Int]:
    var a: Int

    def __init__[*vs: Int](out self):
        var s = Self.n
        comptime for v in vs:
            s += v
        self.a = s


def dup[T: Copyable](x: T) -> T:
    return T.__init__(copy=x)


def make[n: Int, m: Int]() -> Int:
    return D[n].__init__[m]().a


def spread[*mask: Int]() -> Int:
    return V[100].__init__[*mask]().a


def main():
    print(P.__init__(4).a)
    print(Q.__init__().a, Q.__init__[7]().a, Q.__init__[7]().again().a)
    print(Box[2].__init__(3).a)
    print(Box[2].__init__(3).remake().a)
    var p = Pair.__init__(x=1, y=2)
    print(p.again().y)
    var q = Pair.__init__(copy=p)
    print(q.x)
    print(dup(p).y)
    print(D.__init__[5]().a, D[4].__init__[5]().a, D[n=4].__init__[k=5]().a)
    print(D[4].__init__[k=6]("s").a, D[4].__init__[5]().again().a)
    print(make[1, 2]())
    print(V[1].__init__[2, 3, 4]().a, spread[4, 5]())
