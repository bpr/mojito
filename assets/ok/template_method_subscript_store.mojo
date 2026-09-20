# A per-instantiation method clone inherits its checked template's facts when
# the body stores through a subscript of a field of `self`
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `subscript_stores`): a scalar field of the element a reference getter
# yields, or a scalar element a closed setter takes. The subscript is a place
# there, which records its index shape and whether the setter takes the value
# by keyword. The syntax and the setter's declaration decide both, so an
# instance inherits the entry.
struct Entry[T: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var value: Self.T
    var hits: Int

    def __init__(out self, var value: Self.T):
        self.value = value^
        self.hits = 0


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var entries: List[Entry[Self.T]]
    var counts: List[Int]

    def __init__(out self):
        self.entries = List[Entry[Self.T]]()
        self.counts = List[Int]()

    def add(mut self, var value: Self.T):
        self.entries.append(Entry[Self.T](value^))
        self.counts.append(0)

    def reset(mut self, i: Int):
        self.entries[i].hits = 7

    def bump(mut self, i: Int):
        self.entries[i].hits += 1

    def count(mut self, i: Int, n: Int):
        self.counts[i] = n

    def hits_at(self, i: Int) -> Int:
        return self.entries[i].hits

    def count_at(self, i: Int) -> Int:
        return self.counts[i]


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    numbers.reset(0)
    numbers.bump(0)
    numbers.bump(1)
    numbers.count(1, 30)
    print(numbers.hits_at(0), numbers.hits_at(1), numbers.count_at(1))
    var words = Shelf[String]()
    words.add("x")
    words.reset(0)
    words.bump(0)
    words.count(0, 50)
    print(words.hits_at(0), words.count_at(0))
