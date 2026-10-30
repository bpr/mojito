# An explicit `T.__init__(...)` call is the construction `T(...)`: a struct
# name, its application, `Self`, or a type parameter selects among the
# type's initializers, and bracket arguments on a non-parametric struct's
# initializer bind the initializer's own parameters.


@fieldwise_init
struct P(TrivialRegisterPassable):
    var a: Int


struct Q:
    var a: Int

    def __init__(out self):
        self.a = 3

    def __init__[k: Int](out self):
        self.a = k


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


def dup[T: Copyable](x: T) -> T:
    return T.__init__(copy=x)


def main():
    print(P.__init__(4).a)
    print(Q.__init__().a, Q.__init__[7]().a)
    print(Box[2].__init__(3).a)
    print(Box[2].__init__(3).remake().a)
    var p = Pair.__init__(x=1, y=2)
    print(p.again().y)
    var q = Pair.__init__(copy=p)
    print(q.x)
    print(dup(p).y)
