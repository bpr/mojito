# A generic struct's method is checked once, with the struct's parameters
# symbolic. Each per-instantiation clone (`size$y3:Int`) inherits the checked
# template's facts instead of being inferred again (`docs/notes/
# instantiation-from-template.md`, class MethodScalarBody): reads of `self`'s
# scalar fields, runtime parameters, and closed scalar operators.
@fieldwise_init
struct Counter[T: Copyable & Movable & Deinitable](Copyable):
    var item: Self.T
    var count: Int
    var active: Bool

    def size(self) -> Int:
        return self.count

    def doubled(self, extra: Int) -> Int:
        return self.count + self.count + extra

    def is_active(self) -> Bool:
        return self.active


def main():
    var a = Counter(7, 3, True)
    var b = Counter(String("x"), 5, False)
    print(a.size(), a.doubled(1), a.is_active())
    print(b.size(), b.doubled(2), b.is_active())
