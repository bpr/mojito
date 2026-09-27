# Each instance's constructor has its own native symbol: an instance over a
# generic instance (`Bag[List[Int]]`), a variadic initializer keyed by each
# call's element count (`Pack(1, 2)` beside `Pack(1, 2, 3)`), and an instance
# over a loan-carrying argument (`Bag[Span[Int, origin_of(xs)]]`), whose
# constructor is cloned per instance like any other.
struct Bag[T: Copyable & Deinitable](Movable):
    var item: Self.T
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.item = item^
        self.items = List[Self.T]()

    def count(self) -> Int:
        return len(self.items)


struct Pack[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var *values: Self.T):
        self.items = List[Self.T]()
        for value in values:
            self.items.append(value.copy())


def spans(mut xs: List[Int], mut ys: List[Int]) -> Int:
    var a = Bag[Span[Int, origin_of(xs)]](Span(xs))
    var b = Bag[Span[Int, origin_of(ys)]](Span(ys))
    return len(a.item) + len(b.item)


def main():
    var nested = Bag[List[Int]](List[Int]())
    var deeper = Bag[List[List[Int]]](List[List[Int]]())
    var plain = Bag[Int](4)
    print(nested.count(), len(deeper.item), plain.item)
    var two = Pack(1, 2)
    var three = Pack(1, 2, 3)
    var words = Pack[String]("x")
    var lists = Pack[List[Int]](List[Int](), List[Int]())
    print(len(two.items), len(three.items), len(words.items), len(lists.items))
    var xs: List[Int] = [1, 2, 3]
    var ys: List[Int] = [4]
    print(spans(xs, ys))
