# A per-instantiation method clone inherits its checked template's facts when
# the body stores through a subscript of a field of `self`
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `subscript_stores`): a scalar field of the element a reference getter
# yields, an element a declared setter takes — a scalar, or a whole value of
# the setter's own parameter type — or a scalar element stored, whole or
# augmented, through the mutable reference its getter yields. The subscript
# is a place there, which records its index shape and whether the setter
# takes the value by keyword. The syntax and the setter's declaration decide
# both, so an instance inherits the entry; the setter's parameter types
# substitute, and a store through a reference keeps the getter it realized.
# An augmented store through a closed value getter and a setter, a struct
# element's in-place `__iadd__`, and a store on `self` itself derive too: the
# value getter and the dunder are closed calls kept beside the call the site
# selected, and the setter's computed value is keyed at the subscript.
struct Entry[T: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var value: Self.T
    var hits: Int

    def __init__(out self, var value: Self.T):
        self.value = value^
        self.hits = 0


# A mutable-reference getter and no setter: a store writes through the
# reference. The getter names the fields it may return, because returning an
# element's own sub-origin as the container's is a shape upstream refuses.
struct Grid:
    var first: Int
    var second: Int

    def __init__(out self):
        self.first = 0
        self.second = 0

    def __getitem__(ref self, i: Int) -> ref[origin_of(self.first, self.second)] Int:
        if i == 0:
            return self.first
        return self.second


# A value getter and a setter: an augmented store reads the element, then
# writes the result back.
struct Table(Movable):
    var first: Int
    var second: Int

    def __init__(out self):
        self.first = 0
        self.second = 0

    def __getitem__(self, i: Int) -> Int:
        if i == 0:
            return self.first
        return self.second

    def __setitem__(mut self, i: Int, value: Int):
        if i == 0:
            self.first = value
        else:
            self.second = value


# An element whose augmented store selects its in-place dunder.
struct Counter(ImplicitlyCopyable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __iadd__(mut self, k: Int):
        self.n += k


struct Tally(Movable):
    var first: Counter

    def __init__(out self):
        self.first = Counter(0)

    def __getitem__(self, i: Int) -> Counter:
        return self.first

    def __setitem__(mut self, i: Int, value: Counter):
        self.first = value


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var entries: List[Entry[Self.T]]
    var items: List[Self.T]
    var counts: List[Int]
    var buckets: List[List[Int]]
    var grid: Grid
    var table: Table
    var counters: List[Counter]
    var tally: Tally

    def __init__(out self):
        self.entries = List[Entry[Self.T]]()
        self.items = List[Self.T]()
        self.counts = List[Int]()
        self.buckets = List[List[Int]]()
        self.grid = Grid()
        self.table = Table()
        self.counters = List[Counter]()
        self.tally = Tally()

    def __getitem__(self, i: Int) -> Self.T:
        return self.items[i]

    def __setitem__(mut self, i: Int, var value: Self.T):
        self.items[i] = value^

    def add(mut self, var value: Self.T):
        self.entries.append(Entry[Self.T](value))
        self.items.append(value^)
        self.counts.append(0)
        self.buckets.append(List[Int]())
        self.counters.append(Counter(0))

    def reset(mut self, i: Int):
        self.entries[i].hits = 7

    def bump(mut self, i: Int):
        self.entries[i].hits += 1

    def count(mut self, i: Int, n: Int):
        self.counts[i] = n

    # An augmented store through `List.__getitem__`'s mutable reference.
    def bump_count(mut self, i: Int):
        self.counts[i] += 1

    # A whole value of the parameter type, moved through the setter.
    def put_item(mut self, i: Int, var value: Self.T):
        self.items[i] = value^

    # A whole closed value moved through the setter.
    def put_bucket(mut self, i: Int, var bucket: List[Int]):
        self.buckets[i] = bucket^

    # A construction stored through the setter.
    def put_entry(mut self, i: Int, var value: Self.T):
        self.entries[i] = Entry[Self.T](value^)

    # Stores through a getter's mutable reference where no setter exists.
    def set_cell(mut self, i: Int, n: Int):
        self.grid[i] = n

    def bump_cell(mut self, i: Int):
        self.grid[i] += 2

    # An augmented store through a value getter and a setter.
    def bump_table(mut self, i: Int):
        self.table[i] += 1

    # A struct element's in-place dunder, through a mutable reference and
    # through a value getter and a setter.
    def bump_counter(mut self, i: Int):
        self.counters[i] += 3

    def bump_tally(mut self, i: Int):
        self.tally[i] += 4

    # A store on `self` itself, through its own setter.
    def put(mut self, i: Int, var value: Self.T):
        self[i] = value^

    def hits_at(self, i: Int) -> Int:
        return self.entries[i].hits

    def count_at(self, i: Int) -> Int:
        return self.counts[i]

    def item_at(self, i: Int) -> Self.T:
        return self.items[i]

    def bucket_len(self, i: Int) -> Int:
        return len(self.buckets[i])

    def entry_at(self, i: Int) -> Self.T:
        return self.entries[i].value

    def cell_at(self, i: Int) -> Int:
        return self.grid[i]


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    numbers.reset(0)
    numbers.bump(0)
    numbers.bump(1)
    numbers.count(1, 30)
    numbers.bump_count(1)
    numbers.put_item(0, 6)
    numbers.put_bucket(0, [1, 2, 3])
    numbers.put_entry(1, 7)
    numbers.set_cell(0, 9)
    numbers.bump_cell(0)
    print(numbers.hits_at(0), numbers.hits_at(1), numbers.count_at(1))
    print(
        numbers.item_at(0),
        numbers.bucket_len(0),
        numbers.entry_at(1),
        numbers.cell_at(0),
    )
    var words = Shelf[String]()
    words.add("x")
    words.reset(0)
    words.bump(0)
    words.count(0, 50)
    words.bump_count(0)
    words.put_item(0, "y")
    words.put_bucket(0, [1])
    words.put_entry(0, "z")
    words.set_cell(0, 3)
    words.bump_cell(0)
    print(words.hits_at(0), words.count_at(0))
    print(words.item_at(0), words.bucket_len(0), words.entry_at(0), words.cell_at(0))
    numbers.bump_table(1)
    numbers.bump_table(1)
    numbers.bump_counter(1)
    numbers.bump_tally(0)
    numbers.put(1, 8)
    words.bump_table(0)
    words.bump_counter(0)
    words.bump_tally(0)
    words.bump_tally(0)
    words.put(0, "w")
    print(numbers.table[1], numbers.counters[1].n, numbers.tally[0].n, numbers.item_at(1))
    print(words.table[0], words.counters[0].n, words.tally[0].n, words.item_at(0))
