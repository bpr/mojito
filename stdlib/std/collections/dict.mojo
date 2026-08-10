# Dense insertion-ordered dictionary.  The key iterator borrows the entries
# list and yields key references at `element` interior granularity, so
# mutation during iteration is lazily rejected; `keys`/`values`/`items` views
# remain eager value-semantic snapshots by design.

from std.collections.list import List
from std.iterable import Iterable, Iterator, StopIteration
from std.optional import Optional

struct DictEntry[K: Equatable & Copyable & Movable, V: Copyable & Movable](
    Copyable,
    Deinitable where conforms_to(K, Deinitable) and conforms_to(V, Deinitable),
    Movable,
):
    var key: Self.K
    var value: Self.V

    def __init__(out self, key: Self.K, value: Self.V):
        self.key = key
        self.value = value

@fieldwise_init
struct _DictKeyIter[
    iterable_mut: Bool, //,
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
    iterable_origin: Origin[mut=iterable_mut],
](Iterator):
    comptime Element = Self.K

    var src: ref[iterable_origin] List[DictEntry[Self.K, Self.V]]
    var index: Int

    # Optimization hint / compatibility API; exhaustion is StopIteration.
    def __len__(self) -> Int:
        return len(self.src) - self.index

    # Key yields are read-only regardless of the mapping's mutability:
    # writing through a key reference would corrupt the hash invariant.
    def __next__(mut self) raises StopIteration -> ref[
        Origin[mut=False].cast_from[iterable_origin._get_owned_interior["element"]]
    ] Self.K:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r].key

struct Dict[
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
](
    Copyable,
    Deinitable where conforms_to(K, Deinitable) and conforms_to(V, Deinitable),
    Iterable,
    Movable,
    Writable where conforms_to(K, Writable) and conforms_to(V, Writable),
):
    comptime Element = Self.K
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _DictKeyIter[Self.K, Self.V]
    var entries: List[DictEntry[Self.K, Self.V]]

    def __init__(out self):
        self.entries = List[DictEntry[Self.K, Self.V]]()

    def __init__(
        out self,
        var keys: List[Self.K],
        var values: List[Self.V],
        __dict_literal__: NoneType,
    ) where conforms_to(Self.K, Deinitable) and conforms_to(
        Self.V, Deinitable
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
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
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
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var result = List[Self.K]()
        var i = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).key)
            i += 1
        return result^

    def values(self) -> List[Self.V] where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var result = List[Self.V]()
        var i = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).value)
            i += 1
        return result^

    def items(self) -> List[DictEntry[Self.K, Self.V]]:
        return self.entries

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        ref source = self.entries
        return _DictKeyIter[Self.K, Self.V](source, 0)

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
