# A per-instantiation method clone inherits its checked template's facts when
# the body wraps a sibling's view over `ImmOrigin(origin_of(self))`
# (`docs/notes/instantiation-from-template.md`, class MethodBody): the
# construction records the view's slot as bound immutably under the wrapping
# field, which names the struct's own slot and field, so no instance changes
# it. The wrapped view is returned or bound to a local.
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


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def entries(self) -> EntryView[Self.T, ImmOrigin(origin_of(self))]:
        return EntryView[Self.T](
            Pointer(to=self.items).unsafe_origin_cast[ImmOrigin(origin_of(self))](), 0
        )

    def keys(ref self) -> KeyView[Self.T, ImmOrigin(origin_of(self))]:
        return KeyView(self.entries())

    def bound(ref self) -> Int:
        var view = KeyView(self.entries())
        return view.size()


def main():
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]()
    words.add("x")
    print(numbers.keys().size(), words.keys().size())
    print(numbers.bound(), words.bound())
