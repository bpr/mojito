# A per-instantiation method clone inherits its checked template's facts when
# the body hands a reference on as a call argument (`docs/notes/instantiation-
# from-template.md`, class MethodBody, feature `reference_arguments`): a `ref`
# local, a field reached through a reference, or a reference call's result,
# read where it lies by a read parameter, copied into a `var` one, kept by a
# `mut` one, or lent to a hand-written constructor's `ref` parameter. What the
# call records there is decided by what the argument is, never by its type.
struct Entry[V: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var value: Self.V
    var hits: Int

    def __init__(out self, var value: Self.V):
        self.value = value^
        self.hits = 0


struct Counter[m: Bool, //, o: Origin[mut=m]]:
    var count: Int

    def __init__(out self, ref[Self.o] items: List[Int]):
        self.count = len(items)


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var entries: List[Entry[Self.T]]
    var items: List[Self.T]
    var sizes: List[Int]

    def __init__(out self):
        self.entries = List[Entry[Self.T]]()
        self.items = List[Self.T]()
        self.sizes = List[Int]()

    def add(mut self, var value: Self.T):
        self.items.append(value)
        self.entries.append(Entry[Self.T](value^))
        self.sizes.append(len(self.sizes))

    def held(self, entry: Entry[Self.T]) -> Int:
        return len(self.items)

    def read(self, value: Self.T) -> Int:
        return len(self.items)

    def take(self, var value: Self.T) -> Int:
        return len(self.sizes)

    def bump(self, mut hits: Int):
        hits += 1

    def local_entry(self, i: Int) -> Int:
        ref entry = self.entries[i]
        return self.held(entry)

    def call_entry(self, i: Int) -> Int:
        return self.held(self.entries[i])

    def call_item(self, i: Int) -> Int:
        return self.read(self.items[i])

    def local_field(self, i: Int) -> Int:
        ref entry = self.entries[i]
        return self.read(entry.value)

    def call_field(self, i: Int) -> Int:
        return self.read(self.entries[i].value)

    def taken(self, i: Int) -> Int:
        return self.take(self.items[i])

    def kept(mut self, i: Int):
        ref entry = self.entries[i]
        self.bump(entry.hits)
        self.bump(self.entries[i].hits)
        self.bump(self.sizes[i])

    def counted(self) -> Int:
        ref sizes = self.sizes
        var counter = Counter(sizes)
        return 1

    def counted_mut(mut self) -> Int:
        ref sizes = self.sizes
        var counter = Counter(sizes)
        return 2


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]()
    words.add("x")
    print(numbers.local_entry(1), words.local_entry(0), numbers.call_entry(0), words.call_entry(0))
    print(numbers.call_item(1), words.call_item(0), numbers.local_field(0), words.local_field(0))
    print(numbers.call_field(1), words.call_field(0), numbers.taken(0), words.taken(0))
    numbers.kept(1)
    words.kept(0)
    print(numbers.entries[1].hits, words.entries[0].hits, numbers.sizes[1], words.sizes[0])
    print(numbers.counted(), words.counted(), numbers.counted_mut(), words.counted_mut())
