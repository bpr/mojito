# A per-instantiation method clone inherits its checked template's facts when
# the body binds a reference (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `reference_locals`): `ref name = place`, where the
# place is `self`, a field of it, a parameter, a local, or a reference call on
# a field. The binding's type is a reference whose origin names a binding, so
# a template keeps it by owner and an instance gets its own receiver, parameter
# or local back. A read through the binding records the referent, and a copy
# out of it owes the implicit copy at the instance's type.
struct Entry[T: ImplicitlyCopyable & Deinitable](ImplicitlyCopyable):
    var value: Self.T
    var hits: Int

    def __init__(out self, var value: Self.T):
        self.value = value^
        self.hits = 0


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var entries: List[Entry[Self.T]]
    var items: List[Self.T]
    var count: Int

    def __init__(out self):
        self.entries = List[Entry[Self.T]]()
        self.items = List[Self.T]()
        self.count = 0

    def add(mut self, var value: Self.T):
        self.entries.append(Entry[Self.T](value))
        self.items.append(value^)
        self.count += 1

    def hits_at(self, i: Int) -> Int:
        ref entry = self.entries[i]
        return entry.hits

    def value_at(self, i: Int) -> Self.T:
        ref entry = self.entries[i]
        return entry.value

    def item_at(self, i: Int) -> Self.T:
        ref item = self.items[i]
        return item

    def touch(mut self, i: Int):
        ref entry = self.entries[i]
        entry.hits += 1
        entry.hits = entry.hits + 2

    def size(self) -> Int:
        ref held = self.items
        return len(held)

    def mine(self) -> Int:
        ref me = self
        return me.count

    def doubled(self) -> Int:
        var n = self.count
        ref r = n
        return r + n

    def echo(self, ref other: Self.T) -> Self.T:
        ref same = other
        return same

    def peek(
        ref self, i: Int
    ) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        ref item = self.items[i]
        return item


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]()
    words.add("x")
    words.add("y")
    numbers.touch(0)
    words.touch(1)
    print(numbers.hits_at(0), numbers.hits_at(1), words.hits_at(1))
    print(numbers.value_at(1), words.value_at(0))
    print(numbers.item_at(0), words.item_at(1))
    print(numbers.size(), words.size(), numbers.mine(), words.mine())
    print(numbers.doubled(), words.doubled())
    var n = 9
    var text = String("q")
    print(numbers.echo(n), words.echo(text))
    print(numbers.peek(1), words.peek(0))
