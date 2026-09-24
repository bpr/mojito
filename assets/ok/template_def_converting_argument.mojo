# A module-level `def` template's instances inherit its checked facts when an
# argument of a direct call converts through an `@implicit` constructor
# (`docs/notes/instantiation-from-template.md`, class FixedCalls). The
# template's selection of `rank` stands for every instance, as a call inside a
# generic body is bound once; the conversion beneath it is selected again from
# the instance's own source and target types, so a literal keeps picking the
# member of the family its default type names. A compile-time-keyed body
# converts in a `comptime if` arm the same way: each instance re-selects in
# the arm the elaborator kept for it.
struct Label(Copyable, Deinitable, Movable):
    var count: Int

    @implicit
    def __init__(out self, count: Int):
        self.count = count

    @implicit
    def __init__(out self, flag: Bool):
        self.count = 2 if flag else 3


def rank(label: Label) -> Int:
    return label.count


def counted[T: Copyable & Deinitable](value: T) -> Int:
    return rank(4) + rank(True)


def keyed[flag: Bool]() -> Int:
    comptime if flag:
        return rank(4)
    else:
        return rank(True)


def main():
    print(counted[Int](1), counted[String](String("x")))
    print(keyed[True](), keyed[False]())
