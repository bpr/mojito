# A per-instantiation method clone inherits its checked template's facts for
# a call through the struct parameter's bound when the instance's witness
# belongs to a generic struct (`docs/notes/instantiation-from-template.md`,
# obligation 16): a concrete hasher baked into the witness's `[H: Hasher]`
# binder selects the per-call clone keyed by the instance and the call
# together, and a synthesized `Copyable.copy` under a conditional conformance
# is the template's own body, which every instance shares.
from std.hashlib import Hasher
from std.hashlib._ahash import AHasher


@fieldwise_init
struct Wrap[T: Copyable & Deinitable & Hashable](Copyable, Deinitable, Hashable, Movable):
    var x: Self.T

    def __hash__[H: Hasher](self, mut hasher: H):
        self.x.__hash__(hasher)


@fieldwise_init
struct Cell[T: Movable](
    Copyable where conforms_to(T, Copyable),
    Deinitable where conforms_to(T, Deinitable),
    Movable,
):
    var value: Self.T

    def peek(self) -> Int:
        return 1


@fieldwise_init
struct Holder[T: Copyable & Deinitable & Hashable](Movable):
    var item: Self.T

    def feed(self, mut hasher: AHasher[SIMD[DType.uint64, 4](0)]):
        self.item.__hash__(hasher)


@fieldwise_init
struct Shelf[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def dup(self) -> Self.T:
        return self.item.copy()


def digest[T: Copyable & Deinitable & Hashable](holder: Holder[T]) -> UInt64:
    var hasher = AHasher[SIMD[DType.uint64, 4](0)]()
    holder.feed(hasher)
    return hasher^.finish()


def main():
    var ints = Holder[Wrap[Int]](Wrap[Int](3))
    var strings = Holder[Wrap[String]](Wrap[String](String("s")))
    var bare = Holder[Int](3)
    print(digest(ints) == digest(bare), digest(strings) == digest(bare))
    print(digest(strings) == digest(Holder[Wrap[String]](Wrap[String](String("s")))))
    var shelf = Shelf[Cell[Int]](Cell[Int](7))
    var copied = shelf.dup()
    print(copied.value, copied.peek())
    var named = Shelf[Cell[String]](Cell[String](String("n")))
    print(named.dup().value)
