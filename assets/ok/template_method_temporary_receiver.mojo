# A per-instantiation method clone inherits its checked template's facts when
# its body calls a method on a call's temporary result
# (`docs/notes/instantiation-from-template.md`, class MethodBody): a sibling's
# view over `self`, a sibling's owned copy of `Self`, a field's copied list,
# and a field's stripped string view. The temporary's read and its
# destruction are recorded at the call, the same under every instance.
@fieldwise_init
struct EntryView[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var src: Pointer[List[Self.T], Self.o]
    var index: Int

    def size(self) -> Int:
        return len(self.src[]) - self.index

    def past(self, n: Int) -> Int:
        return self.size() - n


struct Shelf[T: ImplicitlyCopyable & Deinitable](Copyable):
    var items: List[Self.T]
    var name: String

    def __init__(out self, name: String):
        self.items = List[Self.T]()
        self.name = name

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def entries(ref self) -> EntryView[Self.T, origin_of(self)]:
        return EntryView[Self.T](
            Pointer(to=self.items).unsafe_origin_cast[origin_of(self)](), 0
        )

    def twin(self) -> Self:
        return self.copy()

    def count(self) -> Int:
        return len(self.items)

    def direct(ref self) -> Int:
        return self.entries().size()

    def skipped(ref self, n: Int) -> Int:
        return self.entries().past(n)

    def summed(ref self) -> Int:
        return self.entries().size() + self.entries().past(1)

    def copied(self) -> Int:
        return self.twin().count()

    def bound(self) -> Int:
        var n = self.twin().count()
        return n * 2

    def total(self) -> Int:
        return self.items.copy().__len__()

    def trimmed(self) -> Int:
        return self.name.strip().byte_length()

    def shout(self) -> String:
        return self.name.strip().upper()


def main():
    var numbers = Shelf[Int]("  ab ")
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]("c ")
    words.add("x")
    print(numbers.direct(), words.direct(), numbers.skipped(1), words.skipped(1))
    print(numbers.summed(), words.summed(), numbers.copied(), words.copied())
    print(numbers.bound(), words.bound(), numbers.total(), words.total())
    print(numbers.trimmed(), words.trimmed(), numbers.shout(), words.shout())
