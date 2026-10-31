# `List(iterable)` copies an iterable's elements into a new list through
# upstream's `__init__[IterableType: Iterable](ref iterable: IterableType,
# out self: List[IterableType.IteratorType[origin_of(iterable)].Element])`,
# whose `out self` follows the argument its refined type reads. A user
# initializer declares the same shape, and a struct's parameter is a
# member of its application (`type_of(bag).T`, `List[Int].T`).
from std.iter import Iterable


struct Bag[T: AnyType](Movable):
    var tag: Int

    def __init__(out self, *, tag: Int):
        self.tag = tag

    def __init__[
        IterableType: Iterable
    ](
        ref iterable: IterableType,
        tag: Int,
        out self: Bag[IterableType.IteratorType[origin_of(iterable)].Element],
    ):
        self = type_of(self)(tag=tag)


def main():
    var xs: List[Int] = [0, 1, 2, 3]
    var ys = List(xs[1:3])
    ys.append(9)
    print(ys, xs)
    var explicit = List[Int](xs[2:4])
    var whole = List(xs)
    whole.append(4)
    print(explicit, whole, xs)
    var words: List[String] = ["a", "b", "c"]
    print(List(words[1:3]))
    var counts: Dict[String, Int] = {"k": 1, "m": 2}
    print(List(counts))
    var bag = Bag(words, 7)
    var element: type_of(bag).T = "z"
    var member: List[Int].T = 5
    print(bag.tag, element, member)
