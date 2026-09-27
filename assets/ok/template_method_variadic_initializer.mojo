# An initializer collecting `var *values` derives from its checked template:
# the collector's type is a pack of the struct's parameter, which each
# instance substitutes, and a `None` default is the same under every
# instance. The literal initializers of `List` and `Set` are such methods.
from std.collections import Set


struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var *values: Self.T, __tag__: NoneType = None):
        self.items = List[Self.T]()
        for value in values:
            self.items.append(value.copy())

    def size(self) -> Int:
        return len(self.items)


def main():
    var a = Bag(1, 2, 3)
    var b = Bag(String("x"), String("y"))
    print(a.size(), b.size())
    var names: List[String] = [String("p"), String("q")]
    var seen: Set[Int] = {1, 2, 2}
    print(len(names), len(seen))
