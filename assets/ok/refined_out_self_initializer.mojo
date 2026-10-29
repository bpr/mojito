# An initializer may refine its `out self` type: the refined type defines the
# struct parameters it spells, a construction whose parameters disagree is no
# candidate, and the initializer's own parameters solve from the constructed
# type or from its arguments.
struct W[n: Int](Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self: W[2], *, two: Int):
        self.v = two * 10 + Self.n

    def __init__[k: Int = 4](out self: W[k], *, gen: Int):
        self.v = gen + k

    def __init__(out self: W[2], *, whole: Int):
        self = W[2](whole + 500)

    @implicit
    def __init__(out self: W[2], flag: Bool):
        self.v = Int(flag) + 40

    def bump(mut self: Self):
        self.v += 1

    def show(self):
        print(Self.n, self.v)


struct P[a: Int, b: Int](Movable):
    var v: Int

    def __init__(out self: P[7, Self.b], *, seven: Int):
        self.v = seven + Self.a * 100 + Self.b * 10


comptime P7 = P[7, 4]


struct Box[T: Copyable & Deinitable](Movable):
    var value: Self.T

    def __init__[U: Copyable & Deinitable](out self: Box[U], var value: U):
        self.value = value^

    def get(self) -> Self.T:
        return self.value.copy()


struct Tag[T: AnyType](Movable):
    var v: Int

    def __init__(out self: Tag[Int], *, i: Int):
        self.v = i

    def __init__[U: AnyType](out self: Tag[List[U]], *, l: Int):
        self.v = l + 1


def take_two(w: W[2]):
    w.show()


def main():
    W[2](two=7).show()          # 2 72
    W(two=8).show()             # 2 82
    var w: W[2] = W(two=9)
    w.bump()
    w.show()                    # 2 93
    W[3](gen=1).show()          # 3 4
    W[7](gen=2).show()          # 7 9
    W(whole=1).show()           # 2 501
    take_two(True)              # 2 41
    print(P[7, 4](seven=3).v)   # 743
    print(P7(seven=1).v)        # 741
    print(P[b=5](seven=0).v)    # 750
    print(Box(5).get())         # 5
    print(Box[String]("hi").get())  # hi
    print(Tag(i=3).v)           # 3
    print(Tag[List[String]](l=4).v)  # 5
