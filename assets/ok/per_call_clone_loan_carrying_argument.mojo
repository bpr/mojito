# A generic method called with a loan-carrying argument of its own
# (`k.keep[Span[Int, origin_of(xs)]](...)`) gets a per-call clone: each origin
# slot of the argument becomes an origin binder the clone declares and infers
# per call from its arguments, so one clone serves every origin of that shape.
# On an instance that carries a loan itself (`Bag[Span[Int, origin_of(ys)]]`),
# the call's binders are numbered after the instance's, since both land on one
# clone; a generic static method binds its binders from its arguments alone.
struct Keeper(Movable):
    var n: Int

    def __init__(out self):
        self.n = 0

    def keep[T: Copyable](mut self, value: T) -> T:
        self.n += 1
        return value.copy()


struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def keep[U: Copyable](self, value: U) -> U:
        return value.copy()

    @staticmethod
    def pass_through[U: Copyable](value: U) -> U:
        return value.copy()


def total(mut xs: List[Int], mut ys: List[Int]) -> Int:
    var k = Keeper()
    var a = k.keep[Span[Int, origin_of(xs)]](Span(xs))
    var b = k.keep[Span[Int, origin_of(ys)]](Span(ys))
    var bag = Bag[Int]()
    var c = bag.keep[Span[Int, origin_of(xs)]](Span(xs))
    var sbag = Bag[Span[Int, origin_of(ys)]]()
    sbag.items.append(Span(ys))
    var d = sbag.keep[Span[Int, origin_of(xs)]](Span(xs))
    var first = a[0] + b[0] * 10 + c[0] * 100 + d[0] * 1000 + k.n * 10000
    ys.append(9)
    var e = Bag[Int].pass_through[Span[Int, origin_of(ys)]](Span(ys))
    return first + e[1] * 100000 + len(ys) * 1000000


def main():
    var xs = List[Int]()
    xs.append(3)
    var ys = List[Int]()
    ys.append(5)
    print(total(xs, ys))
