# A per-instantiation method clone inherits its checked template's facts when
# the body reads a field or calls a closed method through a reference (`docs/
# notes/instantiation-from-template.md`, class MethodBody, feature
# `reference_receivers`): the reference is a reference call's result or a
# `ref` local, and its referent is a struct. The call borrows its receiver
# because of what the receiver is, never because of its type, and the callee
# is the referent's own member or, for a generic referent, that member's clone
# for the instance.
struct Pair(ImplicitlyCopyable):
    var key: Int
    var weight: Int

    def __init__(out self, key: Int, weight: Int):
        self.key = key
        self.weight = weight

    def total(self) -> Int:
        return self.key + self.weight


struct Entry[V: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var value: Self.V
    var hits: Int

    def __init__(out self, var value: Self.V):
        self.value = value^
        self.hits = 0

    def seen(self) -> Int:
        return self.hits

    def bump(mut self, by: Int):
        self.hits += by


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var entries: List[Entry[Self.T]]
    var pairs: List[Pair]

    def __init__(out self):
        self.entries = List[Entry[Self.T]]()
        self.pairs = List[Pair]()

    def add(mut self, var value: Self.T):
        self.pairs.append(Pair(len(self.entries), 2))
        self.entries.append(Entry[Self.T](value^))

    def key_at(self, i: Int) -> Int:
        return self.pairs[i].key

    def total_at(self, i: Int) -> Int:
        return self.pairs[i].total()

    def value_at(self, i: Int) -> Self.T:
        return self.entries[i].value

    def bump_at(mut self, i: Int):
        self.entries[i].bump(3)

    def seen_at(self, i: Int) -> Int:
        ref entry = self.entries[i]
        return entry.seen()

    def touch(mut self, i: Int):
        ref entry = self.entries[i]
        entry.bump(2)
        entry.bump(entry.seen())


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]()
    words.add("x")
    words.add("y")
    print(numbers.key_at(1), words.key_at(0), numbers.total_at(1), words.total_at(0))
    print(numbers.value_at(0), words.value_at(1))
    numbers.bump_at(0)
    words.bump_at(1)
    numbers.touch(0)
    words.touch(0)
    print(numbers.seen_at(0), numbers.seen_at(1), words.seen_at(0), words.seen_at(1))
