# A per-instantiation method clone inherits its checked template's facts when
# the body calls a sibling method whose result is a view over `self`
# (`docs/notes/instantiation-from-template.md`, class MethodBody): the call's
# `BorrowViewResult` loan and the origin its result binds name the receiver,
# which no instance changes. The view is returned as is, wrapped by a
# fieldwise or a hand-written constructor, or bound to a local.
@fieldwise_init
struct EntryView[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var src: Pointer[List[Self.T], Self.o]
    var index: Int

    def size(self) -> Int:
        return len(self.src[]) - self.index


@fieldwise_init
struct KeyView[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var iter: EntryView[Self.T, Self.o]

    def size(self) -> Int:
        return self.iter.size()


struct Counted[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var iter: EntryView[Self.T, Self.o]
    var extra: Int

    def __init__(out self, iter: EntryView[Self.T, Self.o]):
        self.iter = iter
        self.extra = 10

    def size(self) -> Int:
        return self.iter.size() + self.extra


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    comptime KeysType[m: Bool, //, o: Origin[mut=m]] = KeyView[Self.T, o]

    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def entries(ref self) -> EntryView[Self.T, origin_of(self)]:
        return EntryView[Self.T](
            Pointer(to=self.items).unsafe_origin_cast[origin_of(self)](), 0
        )

    def view(ref self) -> EntryView[Self.T, origin_of(self)]:
        return self.entries()

    def keys(ref self) -> KeyView[Self.T, origin_of(self)]:
        return KeyView(self.entries())

    def aliased(ref self) -> Self.KeysType[origin_of(self)]:
        return KeyView(self.entries())

    def counted(ref self) -> Counted[Self.T, origin_of(self)]:
        return Counted(self.entries())

    def bound(ref self) -> Int:
        var view = self.entries()
        return view.size()


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]()
    words.add("x")
    print(numbers.view().size(), words.view().size(), numbers.keys().size(), words.keys().size())
    print(numbers.aliased().size(), words.aliased().size(), numbers.counted().size(), words.counted().size())
    print(numbers.bound(), words.bound())
