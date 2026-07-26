from std.collections.list import List, _ListIter
from std.iterable import Iterable

struct Set[T: Equatable & Copyable & Movable](
    Copyable,
    ImplicitlyDeletable where conforms_to(T, ImplicitlyDeletable),
    Iterable,
    Movable,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime Iter = _ListIter[Self.T]

    # A dense list deliberately preserves display and iteration insertion order.
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def __init__(out self, var *values: Self.T, __set_literal__: NoneType) where conforms_to(
        Self.T, ImplicitlyDeletable
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
        Self.T, ImplicitlyDeletable
    ):
        if not (value in self):
            self.items.append(value^)

    def __len__(self) -> Int:
        return len(self.items)

    def __iter__(self) -> _ListIter[Self.T]:
        return self.items.__iter__()

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
