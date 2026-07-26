# Dense insertion-ordered dictionary.  Views remain eager value-semantic
# snapshots; the collection itself is nominal and protocol-driven.

from std.collections.list import List
from std.iterable import Iterable, Iterator, StopIteration
from std.optional import Optional

struct DictEntry[K: Equatable & Copyable & Movable, V: Copyable & Movable](
    Copyable,
    ImplicitlyDeletable where conforms_to(K, ImplicitlyDeletable) and conforms_to(V, ImplicitlyDeletable),
    Movable,
):
    var key: Self.K
    var value: Self.V

    def __init__(out self, key: Self.K, value: Self.V):
        self.key = key
        self.value = value

struct _DictKeyIter[
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
](Iterator):
    comptime Element = Self.K
    var entries: List[DictEntry[Self.K, Self.V]]
    var index: Int

    def __init__(out self, entries: List[DictEntry[Self.K, Self.V]]):
        self.entries = entries
        self.index = 0

    def __len__(self) -> Int:
        return len(self.entries) - self.index

    def __next__(mut self) raises StopIteration -> Self.K:
        if self.index >= len(self.entries):
            raise StopIteration()
        var result = self.entries._get_copy(self.index).key
        self.index += 1
        return result^

struct Dict[
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
](
    Copyable,
    ImplicitlyDeletable where conforms_to(K, ImplicitlyDeletable) and conforms_to(V, ImplicitlyDeletable),
    Iterable,
    Movable,
    Writable where conforms_to(K, Writable) and conforms_to(V, Writable),
):
    comptime Element = Self.K
    comptime Iter = _DictKeyIter[Self.K, Self.V]
    var entries: List[DictEntry[Self.K, Self.V]]

    def __init__(out self):
        self.entries = List[DictEntry[Self.K, Self.V]]()

    def __init__(
        out self,
        var keys: List[Self.K],
        var values: List[Self.V],
        __dict_literal__: NoneType,
    ) where conforms_to(Self.K, ImplicitlyDeletable) and conforms_to(
        Self.V, ImplicitlyDeletable
    ):
        self.entries = List[DictEntry[Self.K, Self.V]]()
        var i = 0
        while i < len(keys):
            self[keys[i]] = values[i]
            i += 1

    def __init__(out self, *, copy: Self):
        self.entries = copy.entries

    def copy(self) -> Self:
        return Dict[Self.K, Self.V](copy: self)

    def __init__(out self, *, deinit move: Self):
        self.entries = move.entries^

    def find_index(self, key: Self.K) -> Int:
        var i = 0
        while i < len(self.entries):
            if self.entries._get_copy(i).key == key:
                return i
            i += 1
        return -1

    def __contains__(self, key: Self.K) -> Bool:
        return self.find_index(key) >= 0

    def __getitem__(
        ref self, key: Self.K
    ) raises -> ref[origin_of(self)._get_owned_interior["value"]] Self.V:
        var i = self.find_index(key)
        if i >= 0:
            return self.entries[i].value
        raise Error("missing key")

    def __setitem__(mut self, key: Self.K, value: Self.V) where conforms_to(
        Self.K, ImplicitlyDeletable
    ) and conforms_to(Self.V, ImplicitlyDeletable):
        var i = self.find_index(key)
        if i >= 0:
            self.entries[i] = DictEntry[Self.K, Self.V](key, value)
        else:
            self.entries.append(DictEntry[Self.K, Self.V](key, value))

    def get(self, key: Self.K) -> Optional[Self.V]:
        var i = self.find_index(key)
        if i >= 0:
            return Optional[Self.V](self.entries._get_copy(i).value, True)
        return Optional[Self.V]()

    def get(self, key: Self.K, default: Self.V) -> Self.V:
        var i = self.find_index(key)
        if i >= 0:
            return self.entries._get_copy(i).value
        return default

    def __len__(self) -> Int:
        return len(self.entries)

    def keys(self) -> List[Self.K] where conforms_to(
        Self.K, ImplicitlyDeletable
    ) and conforms_to(Self.V, ImplicitlyDeletable):
        var result = List[Self.K]()
        var i = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).key)
            i += 1
        return result^

    def values(self) -> List[Self.V] where conforms_to(
        Self.K, ImplicitlyDeletable
    ) and conforms_to(Self.V, ImplicitlyDeletable):
        var result = List[Self.V]()
        var i = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).value)
            i += 1
        return result^

    def items(self) -> List[DictEntry[Self.K, Self.V]]:
        return self.entries

    def __iter__(self) -> _DictKeyIter[Self.K, Self.V]:
        return _DictKeyIter[Self.K, Self.V](self.entries)

    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.K, Writable
    ) and conforms_to(Self.V, Writable):
        writer.write("{")
        var i = 0
        while i < len(self.entries):
            if i > 0:
                writer.write(", ")
            writer.write(
                self.entries._get_copy(i).key,
                ": ",
                self.entries._get_copy(i).value,
            )
            i += 1
        writer.write("}")
