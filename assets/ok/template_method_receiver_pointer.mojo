# A per-instantiation method clone inherits its checked template's facts when
# the body hands a construction a pointer to `self` or a field of it rebound
# to the whole receiver, `Pointer(to=self.items).unsafe_origin_cast[origin_of(self)]()`
# (`docs/notes/instantiation-from-template.md`, class MethodBody). Both
# provenances are the receiver's own place, which an instance roots at its own
# `self`, so a read or a mutable receiver, and a fieldwise or a hand-written
# constructor, derive alike; `Set` and `Dict` build their borrowed iterators
# this way.
from std.collections import Set


@fieldwise_init
struct Cursor[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var src: Pointer[List[Self.T], Self.o]
    var index: Int

    def size(self) -> Int:
        return len(self.src[]) - self.index


struct Offset[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var src: Pointer[List[Self.T], Self.o]
    var extra: Int

    def __init__(out self, src: Pointer[List[Self.T], Self.o], extra: Int):
        self.src = src
        self.extra = extra

    def size(self) -> Int:
        return len(self.src[]) + self.extra


@fieldwise_init
struct Whole[m: Bool, //, T: ImplicitlyCopyable & Deinitable, o: Origin[mut=m]](
    ImplicitlyCopyable
):
    var src: Pointer[Shelf[Self.T], Self.o]

    def size(self) -> Int:
        return len(self.src[].items)


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def cursor(ref self) -> Cursor[Self.T, origin_of(self)]:
        return Cursor[Self.T](
            Pointer(to=self.items).unsafe_origin_cast[origin_of(self)](), 1
        )

    def offset(ref self) -> Offset[Self.T, origin_of(self)]:
        return Offset[Self.T](
            Pointer(to=self.items).unsafe_origin_cast[origin_of(self)](), 10
        )

    def whole(ref self) -> Whole[Self.T, origin_of(self)]:
        return Whole[Self.T](
            Pointer(to=self).unsafe_origin_cast[origin_of(self)]()
        )

    def edit(mut self) -> Cursor[Self.T, origin_of(self)]:
        return Cursor[Self.T](
            Pointer(to=self.items).unsafe_origin_cast[origin_of(self)](), 0
        )


def main() raises:
    var numbers = Shelf[Int]()
    numbers.add(4)
    numbers.add(5)
    var words = Shelf[String]()
    words.add("x")
    print(numbers.cursor().size(), words.cursor().size())
    print(numbers.offset().size(), words.offset().size())
    print(numbers.edit().size(), words.edit().size())
    print(numbers.whole().size(), words.whole().size())
    var seen = Set[Int](3, 4)
    var tags = Set[String](String("a"))
    var total = 0
    for value in seen:
        total += value
    for tag in tags:
        print(tag, total)
    var counts = Dict[String, Int]()
    counts["k"] = 2
    for key in counts:
        print(key, counts[key])
