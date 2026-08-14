from std.collections.list import List, _ListIter
from std.iterable import Iterable
from std.optional import Optional

struct Set[T: Equatable & Copyable & Movable](
    Copyable,
    Deinitable where conforms_to(T, Deinitable),
    Iterable,
    Movable,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ListIter[Self.T]

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

    # Drain every element through the caller-supplied consuming handler,
    # leaving the set empty and reusable.
    def clear_with(mut self, elt_handler: def(deinit element: Self.T) capturing[_], /):
        while len(self.items) > 0:
            elt_handler(self.items.pop())

    # Consuming teardown: `clear_with` under a consumed receiver.
    def deinit_with(deinit self, elt_handler: def(deinit element: Self.T) capturing[_], /):
        while len(self.items) > 0:
            elt_handler(self.items.pop())

    def __len__(self) -> Int:
        return len(self.items)

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        # Construct the borrowed iterator directly: an explicit
        # `self.items.__iter__()` call is ambiguous between the borrowed and
        # owned List overloads.
        ref source = self.items
        return _ListIter[Self.T](source, 0)

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
