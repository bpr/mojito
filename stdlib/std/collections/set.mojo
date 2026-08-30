from std.collections.list import List, _ListOwnedIter
from std.iterable import Iterable, IterableOwned, Iterator, StopIteration
from std.memory import unsafe_alloc
from std.optional import Optional

@fieldwise_init
struct _SetIter[
    iterable_mut: Bool, //,
    T: Hashable & Equatable & Copyable & Movable,
    iterable_origin: Origin[mut=iterable_mut],
](Iterator):
    comptime Element = Self.T

    var src: ref[iterable_origin] List[Self.T]
    var index: Int

    # Optimization hint / compatibility API; exhaustion is StopIteration.
    def __len__(self) -> Int:
        return len(self.src) - self.index

    # Element yields are read-only regardless of the set's mutability:
    # writing through an element reference would corrupt the hash and
    # uniqueness invariants.
    def __next__(mut self) raises StopIteration -> ref[
        Origin[mut=False].cast_from[Self.iterable_origin._get_owned_interior["element"]]
    ] Self.T:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

struct Set[T: Hashable & Equatable & Copyable & Movable](
    Copyable,
    Deinitable where conforms_to(T, Deinitable),
    Equatable,
    Iterable,
    IterableOwned where conforms_to(T, Deinitable),
    Movable,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _SetIter[Self.T, iterable_origin]
    comptime IteratorOwnedType = _ListOwnedIter[Self.T]

    # A dense list deliberately preserves display and iteration insertion order.
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def __init__(out self, var *values: Self.T, __set_literal__: NoneType = None) where conforms_to(
        Self.T, Deinitable
    ):
        self.items = List[Self.T]()
        for var value in values^:
            self.add(value^)

    def __init__(out self, *, copy: Self):
        self.items = copy.items

    def copy(self) -> Self:
        return Set[Self.T](copy: self)

    def __init__(out self, *, deinit move: Self):
        self.items = move.items^

    def __contains__(self, value: Self.T) -> Bool:
        return value in self.items

    def contains(self, value: Self.T) -> Bool:
        return value in self

    def add(mut self, var value: Self.T) where conforms_to(
        Self.T, Deinitable
    ):
        if not (value in self):
            self.items.append(value^)

    # Displacement-returning insertion: replacing an equal element returns the
    # previously stored one, a fresh element returns an empty Optional.
    def insert(mut self, var value: Self.T) -> Optional[Self.T] where conforms_to(
        Self.T, Deinitable
    ):
        var i = self._find(value)
        if i >= 0:
            var displaced = Optional[Self.T](self.items._get_copy(i))
            self.items[i] = value^
            return displaced^
        self.items.append(value^)
        return Optional[Self.T]()

    # Remove `value`, raising when it is absent (`discard` is the silent
    # spelling).
    def remove(mut self, value: Self.T) raises where conforms_to(
        Self.T, Deinitable
    ):
        var i = self._find(value)
        if i < 0:
            raise Error("missing element")
        _ = self.items.pop(i)

    def discard(mut self, value: Self.T) where conforms_to(Self.T, Deinitable):
        var i = self._find(value)
        if i >= 0:
            _ = self.items.pop(i)

    # Remove and return the last-inserted element, raising when empty.
    def pop(mut self) raises -> Self.T:
        if len(self.items) == 0:
            raise Error("Pop on empty set")
        return self.items.pop(len(self.items) - 1)

    def clear(mut self) where conforms_to(Self.T, Deinitable):
        self.items.clear()

    # Drain every element front-to-back through the caller-supplied consuming
    # handler, leaving the set empty and reusable.
    def clear_with(mut self, elt_handler: def(var element: Self.T) capturing[_], /):
        while len(self.items) > 0:
            elt_handler(self.items.pop(0))

    # Consuming teardown: `clear_with` under a consumed receiver.
    def deinit_with(deinit self, elt_handler: def(var element: Self.T) capturing[_], /):
        while len(self.items) > 0:
            elt_handler(self.items.pop(0))

    # In-place union with `other`.
    def update(mut self, other: Self) where conforms_to(Self.T, Deinitable):
        var i = 0
        while i < len(other.items):
            self.add(other.items._get_copy(i))
            i += 1

    def intersection_update(mut self, other: Self) where conforms_to(
        Self.T, Deinitable
    ):
        var result = self.intersection(other)
        self.items = result.items

    def difference_update(mut self, other: Self) where conforms_to(
        Self.T, Deinitable
    ):
        var result = self.difference(other)
        self.items = result.items

    def symmetric_difference_update(mut self, other: Self) where conforms_to(
        Self.T, Deinitable
    ):
        var result = self.symmetric_difference(other)
        self.items = result.items

    def union(self, other: Self) -> Self where conforms_to(Self.T, Deinitable):
        var result = self.copy()
        result.update(other)
        return result^

    def intersection(self, other: Self) -> Self where conforms_to(
        Self.T, Deinitable
    ):
        var result = Set[Self.T]()
        var i = 0
        while i < len(self.items):
            if other.contains(self.items._get_copy(i)):
                result.add(self.items._get_copy(i))
            i += 1
        return result^

    def difference(self, other: Self) -> Self where conforms_to(
        Self.T, Deinitable
    ):
        var result = Set[Self.T]()
        var i = 0
        while i < len(self.items):
            if not other.contains(self.items._get_copy(i)):
                result.add(self.items._get_copy(i))
            i += 1
        return result^

    def symmetric_difference(self, other: Self) -> Self where conforms_to(
        Self.T, Deinitable
    ):
        var result = self.difference(other)
        result.update(other.difference(self))
        return result^

    def __and__(self, other: Self) -> Self where conforms_to(Self.T, Deinitable):
        return self.intersection(other)

    def __or__(self, other: Self) -> Self where conforms_to(Self.T, Deinitable):
        return self.union(other)

    def __sub__(self, other: Self) -> Self where conforms_to(Self.T, Deinitable):
        return self.difference(other)

    def __xor__(self, other: Self) -> Self where conforms_to(Self.T, Deinitable):
        return self.symmetric_difference(other)

    def __isub__(mut self, other: Self) where conforms_to(Self.T, Deinitable):
        self.difference_update(other)

    def issubset(self, other: Self) -> Bool:
        if len(self.items) > len(other.items):
            return False
        var i = 0
        while i < len(self.items):
            if not other.contains(self.items._get_copy(i)):
                return False
            i += 1
        return True

    def issuperset(self, other: Self) -> Bool:
        return other.issubset(self)

    def isdisjoint(self, other: Self) -> Bool:
        var i = 0
        while i < len(self.items):
            if other.contains(self.items._get_copy(i)):
                return False
            i += 1
        return True

    def __eq__(self, other: Self) -> Bool:
        return len(self.items) == len(other.items) and self.issubset(other)

    def __ne__(self, other: Self) -> Bool:
        return not (self == other)

    def __le__(self, other: Self) -> Bool:
        return self.issubset(other)

    def __ge__(self, other: Self) -> Bool:
        return self.issuperset(other)

    def __lt__(self, other: Self) -> Bool:
        return len(self.items) < len(other.items) and self.issubset(other)

    def __gt__(self, other: Self) -> Bool:
        return len(self.items) > len(other.items) and self.issuperset(other)

    def __bool__(self) -> Bool:
        return len(self.items) > 0

    def __len__(self) -> Int:
        return len(self.items)

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        # Construct the borrowed iterator directly: an explicit
        # `self.items.__iter__()` call is ambiguous between the borrowed and
        # owned List overloads.
        ref source = self.items
        return _SetIter[Self.T](source, 0)

    def __iter__(var self) -> _ListOwnedIter[Self.T] where conforms_to(
        Self.T, Deinitable
    ):
        # Consuming iteration drains the backing list in insertion order,
        # constructing the owned iterator directly like the borrowed overload.
        var result = _ListOwnedIter[Self.T](self.items.data, self.items.size)
        self.items.data = unsafe_alloc[Self.T](0)
        self.items.size = 0
        self.items.cap = 0
        return result^

    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.T, Writable
    ):
        if len(self) == 0:
            writer.write("{}")
            return
        writer.write("{")
        var i = 0
        while i < len(self.items):
            if i > 0:
                writer.write(", ")
            writer.write(self.items[i])
            i += 1
        writer.write("}")

    # Library-private membership index (Set owns its equality scan rather
    # than leaning on List.index's raising contract).
    def _find(self, value: Self.T) -> Int:
        var i = 0
        while i < len(self.items):
            if self.items._get_copy(i) == value:
                return i
            i += 1
        return -1
