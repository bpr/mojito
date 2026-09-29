# A call returning a mutable reference is an assignment target: `=` and an
# augmented operator write through the returned reference, as does a field
# store below it, for a module `def` and a method alike.
@fieldwise_init
struct Point(Copyable, Movable):
    var x: Int
    var y: Int

    def x_ref(mut self) -> ref[origin_of(self.x)] Int:
        return self.x


@fieldwise_init
struct Named(Copyable, Movable):
    var name: String

    def name_ref(mut self) -> ref[origin_of(self.name)] String:
        return self.name


def bump(ref a: Int) -> ref[origin_of(a)] Int:
    return a


def pick(ref p: Point) -> ref[origin_of(p)] Point:
    return p


def named(ref n: Named) -> ref[origin_of(n)] Named:
    return n


def text(ref t: String) -> ref[origin_of(t)] String:
    return t


def main():
    var k = 5
    bump(k) = 9
    print(k)
    bump(k) += 1
    print(k)
    var p = Point(1, 2)
    p.x_ref() = 7
    p.x_ref() *= 3
    print(p.x)
    pick(p).y = 40
    pick(p).y += 2
    print(p.y)
    pick(p) = Point(5, 6)
    print(p.x, p.y)
    pick(p).x_ref() -= 1
    print(p.x)
    var s = String("a")
    text(s) = String("bc")
    print(s)
    var n = Named(String("a"))
    n.name_ref() += "b"
    named(n).name += "c"
    named(n).name_ref() += "d"
    print(n.name)
