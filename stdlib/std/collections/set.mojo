from std.collections.list import List, _ListIter, _ListOwnedIter
from std.iterable import Iterable, IterableOwned
from std.memory import unsafe_alloc
from std.optional import Optional

struct Set[T: Equatable & Copyable & Movable](
    Copyable,
    Deinitable where conforms_to(T, Deinitable),
    Iterable,
    IterableOwned where conforms_to(T, Deinitable),
    Movable,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ListIter[Self.T]
    comptime IteratorOwnedType = _ListOwnedIter[Self.T]

    # A dense list deliberately preserves display and iteration insertion order.
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def __init__(out self, var *values: Self.T, __set_literal__: NoneType) where conforms_to(
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
        var i = self.items.index(value)
        if i >= 0:
            var displaced = Optional[Self.T](self.items._get_copy(i))
            self.items[i] = value^
            return displaced^
        self.items.append(value^)
        return Optional[Self.T]()

    # Drain every element front-to-back through the caller-supplied consuming
    # handler, leaving the set empty and reusable.
    def clear_with(mut self, elt_handler: def(var element: Self.T) capturing[_], /):
        while len(self.items) > 0:
            elt_handler(self.items.pop(0))

    # Consuming teardown: `clear_with` under a consumed receiver.
    def deinit_with(deinit self, elt_handler: def(var element: Self.T) capturing[_], /):
        while len(self.items) > 0:
            elt_handler(self.items.pop(0))

    def __len__(self) -> Int:
        return len(self.items)

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        # Construct the borrowed iterator directly: an explicit
        # `self.items.__iter__()` call is ambiguous between the borrowed and
        # owned List overloads.
        ref source = self.items
        return _ListIter[Self.T](source, 0)

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
