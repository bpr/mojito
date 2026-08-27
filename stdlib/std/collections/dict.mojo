# A hash-backed, insertion-ordered dictionary.
#
# Dense entries preserve insertion order; a nested-list index maps each hash
# bucket to entry positions, doubling when the load factor reaches one. The
# key iterator borrows the entries list and yields key references at
# `element` interior granularity, so mutation during iteration is lazily
# rejected; `keys`/`values`/`items` return self-iterable snapshot iterators
# that expose no indexing or `len`, matching upstream's non-indexable view
# surface (the snapshot itself is a recorded divergence from upstream's
# borrowing views).

from std.collections.list import List, _ListOwnedIter
from std.hashing import bucket_index
from std.iterable import Iterable, Iterator, StopIteration
from std.memory import unsafe_alloc
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

# The `keys`/`values`/`items` view iterator: owns a dense element snapshot
# taken at call time and yields elements by value. It exposes no indexing
# and no `len`, matching upstream's non-indexable view surface; unlike
# upstream's borrowing views, source mutation after the call is not tracked
# against it (recorded divergence — the snapshot iterates unperturbed).
struct _DictSnapshotIter[T: Copyable & Movable](
    Copyable,
    Deinitable where conforms_to(T, Deinitable),
    Iterator,
    Movable,
):
    comptime Element = Self.T

    var items: List[Self.T]
    var index: Int

    def __init__(out self, var items: List[Self.T]):
        self.items = items^
        self.index = 0

    def __init__(out self, *, copy: Self):
        self.items = List[Self.T](copy: copy.items)
        self.index = copy.index

    def copy(self) -> Self:
        return _DictSnapshotIter[Self.T](copy: self)

    def __init__(out self, *, deinit move: Self):
        self.items = move.items^
        self.index = move.index

    # The view iterates itself; a stored view restarts from its own cursor.
    def __iter__(ref self) -> Self:
        return _DictSnapshotIter[Self.T](copy: self)

    def __next__(mut self) raises StopIteration -> Self.T:
        if self.index >= len(self.items):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.items._get_copy(r)

struct Dict[
    K: Hashable & Equatable & Copyable & Movable,
    V: Copyable & Movable,
](
    Copyable,
    Deinitable where conforms_to(K, Deinitable) and conforms_to(V, Deinitable),
    Equatable where conforms_to(V, Equatable),
    Iterable,
    Movable,
    Writable where conforms_to(K, Writable) and conforms_to(V, Writable),
):
    comptime Element = Self.K
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _DictKeyIter[Self.K, Self.V]
    var entries: List[DictEntry[Self.K, Self.V]]
    var index: List[List[Int]]
    var nbuckets: Int

    def __init__(out self):
        self.entries = List[DictEntry[Self.K, Self.V]]()
        self.index = List[List[Int]]()
        self.nbuckets = 8
        var i = 0
        while i < self.nbuckets:
            self.index.append(List[Int]())
            i += 1

    # Capacity is a bucket-count hint: the index starts wide enough that the
    # first `capacity` insertions never rehash.
    def __init__(out self, *, capacity: Int):
        self.entries = List[DictEntry[Self.K, Self.V]]()
        self.index = List[List[Int]]()
        self.nbuckets = 8
        while self.nbuckets < capacity:
            self.nbuckets *= 2
        var i = 0
        while i < self.nbuckets:
            self.index.append(List[Int]())
            i += 1

    def __init__(
        out self,
        var keys: List[Self.K],
        var values: List[Self.V],
        __dict_literal__: NoneType,
    ) where conforms_to(Self.K, Deinitable) and conforms_to(
        Self.V, Deinitable
    ):
        self.entries = List[DictEntry[Self.K, Self.V]]()
        self.index = List[List[Int]]()
        self.nbuckets = 8
        var b = 0
        while b < self.nbuckets:
            self.index.append(List[Int]())
            b += 1
        var i = 0
        while i < len(keys):
            self[keys[i]] = values[i]
            i += 1

    def __init__(out self, *, copy: Self):
        self.entries = List[DictEntry[Self.K, Self.V]](copy: copy.entries)
        self.index = List[List[Int]](copy: copy.index)
        self.nbuckets = copy.nbuckets

    def copy(self) -> Self:
        return Dict[Self.K, Self.V](copy: self)

    def __init__(out self, *, deinit move: Self):
        self.entries = move.entries^
        self.index = move.index^
        self.nbuckets = move.nbuckets

    def find_index(self, key: Self.K) -> Int:
        var bucket = bucket_index(key, self.nbuckets)
        for entry_index in self.index._get_copy(bucket):
            if self.entries._get_copy(entry_index).key == key:
                return entry_index
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
            self._append_new(DictEntry[Self.K, Self.V](key, value))

    # Displacement-returning insertion: replacing an existing key moves the
    # previous entry (key and value) out and returns it; a fresh key returns
    # an empty Optional. Nothing is destroyed in place, so no `Deinitable`
    # bound is required.
    def insert(mut self, var key: Self.K, var value: Self.V) -> Optional[
        DictEntry[Self.K, Self.V]
    ]:
        var i = self.find_index(key)
        if i >= 0:
            var displaced = Optional[DictEntry[Self.K, Self.V]](
                self.entries.data.unsafe_offset(i).unsafe_take_pointee()
            )
            self.entries.data[i] = DictEntry[Self.K, Self.V](key, value)
            return displaced^
        self._append_new(DictEntry[Self.K, Self.V](key^, value^))
        return Optional[DictEntry[Self.K, Self.V]]()

    # Drain every entry into an owned iterator. Upstream returns a lazily
    # draining borrowed iterator; Mojito moves the entries out eagerly (the
    # dictionary is observably empty as soon as this returns — recorded
    # divergence) and the returned iterator owns them.
    def take_items(mut self) -> _ListOwnedIter[
        DictEntry[Self.K, Self.V]
    ] where conforms_to(Self.K, Deinitable) and conforms_to(
        Self.V, Deinitable
    ):
        var result = _ListOwnedIter[DictEntry[Self.K, Self.V]](
            self.entries.data, self.entries.size
        )
        self.entries.data = unsafe_alloc[DictEntry[Self.K, Self.V]](0)
        self.entries.size = 0
        self.entries.cap = 0
        self._reset_index()
        return result^

    # Remove the entry for `key` and return its value; the discarded key
    # needs `Deinitable`. Missing keys raise (`pop(key, default)` instead
    # returns the default).
    def pop(mut self, key: Self.K) raises -> Self.V where conforms_to(
        Self.K, Deinitable
    ):
        var i = self.find_index(key)
        if i < 0:
            raise Error("missing key")
        var entry = self.entries.pop(i)
        self._reindex()
        return entry.value^

    def pop(mut self, key: Self.K, var default: Self.V) -> Self.V where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var i = self.find_index(key)
        if i < 0:
            return default^
        var entry = self.entries.pop(i)
        self._reindex()
        return entry.value^

    # Remove and return the last-inserted entry (LIFO), raising when empty.
    def popitem(mut self) raises -> DictEntry[Self.K, Self.V]:
        if len(self.entries) == 0:
            raise Error("empty dictionary")
        var entry = self.entries.pop(len(self.entries) - 1)
        self._reindex()
        return entry^

    # Return a reference to the value for `key`, inserting `default` first
    # when the key is absent.
    def setdefault(
        mut self, var key: Self.K, var default: Self.V
    ) -> ref[
        origin_of(self)._get_owned_interior["value"]
    ] Self.V where conforms_to(Self.K, Deinitable) and conforms_to(
        Self.V, Deinitable
    ):
        var i = self.find_index(key)
        if i < 0:
            i = len(self.entries)
            self._append_new(DictEntry[Self.K, Self.V](key^, default^))
        return self.entries[i].value

    # Copy every entry of `other` into self, overwriting existing keys.
    def update(mut self, other: Self, /) where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var i = 0
        while i < len(other.entries):
            self[other.entries._get_copy(i).key] = other.entries._get_copy(i).value
            i += 1

    # Destroy every entry in place, leaving the dictionary empty.
    def clear(mut self) where conforms_to(Self.K, Deinitable) and conforms_to(
        Self.V, Deinitable
    ):
        self.entries.clear()
        self._reset_index()

    # Drain every entry front-to-back through the caller-supplied consuming
    # handler, leaving the dictionary empty and reusable.
    def clear_with(
        mut self,
        elt_handler: def(var key: Self.K, var value: Self.V) capturing[_],
        /,
    ):
        while len(self.entries) > 0:
            var entry = self.entries.pop(0)
            elt_handler(entry.key^, entry.value^)
        self._reset_index()

    # Consuming teardown: `clear_with` under a consumed receiver.
    def deinit_with(
        deinit self,
        elt_handler: def(var key: Self.K, var value: Self.V) capturing[_],
        /,
    ):
        while len(self.entries) > 0:
            var entry = self.entries.pop(0)
            elt_handler(entry.key^, entry.value^)

    def get(self, key: Self.K) -> Optional[Self.V]:
        var i = self.find_index(key)
        if i >= 0:
            return Optional[Self.V](self.entries._get_copy(i).value)
        return Optional[Self.V]()

    def get(self, key: Self.K, default: Self.V) -> Self.V:
        var i = self.find_index(key)
        if i >= 0:
            return self.entries._get_copy(i).value
        return default

    def __len__(self) -> Int:
        return len(self.entries)

    # Snapshot views: each returns a self-iterable, non-indexable iterator
    # over elements copied at call time (see `_DictSnapshotIter`).
    def keys(self) -> _DictSnapshotIter[Self.K] where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var result = List[Self.K]()
        var i = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).key)
            i += 1
        return _DictSnapshotIter[Self.K](result^)

    def values(self) -> _DictSnapshotIter[Self.V] where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var result = List[Self.V]()
        var i = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).value)
            i += 1
        return _DictSnapshotIter[Self.V](result^)

    def items(self) -> _DictSnapshotIter[DictEntry[Self.K, Self.V]] where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        return _DictSnapshotIter[DictEntry[Self.K, Self.V]](
            List[DictEntry[Self.K, Self.V]](copy: self.entries)
        )

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        ref source = self.entries
        return _DictKeyIter[Self.K, Self.V](source, 0)

    def __bool__(self) -> Bool:
        return len(self.entries) > 0

    # Equal when the same keys map to equal values; insertion order is not
    # part of equality.
    def __eq__(self, other: Self) -> Bool where conforms_to(Self.V, Equatable):
        if len(self.entries) != len(other.entries):
            return False
        var i = 0
        while i < len(self.entries):
            var j = other.find_index(self.entries._get_copy(i).key)
            if j < 0:
                return False
            if not (
                other.entries._get_copy(j).value
                == self.entries._get_copy(i).value
            ):
                return False
            i += 1
        return True

    def __ne__(self, other: Self) -> Bool where conforms_to(Self.V, Equatable):
        return not (self == other)

    # Merge: `other`'s entries overwrite shared keys in the copied result.
    def __or__(self, other: Self) -> Self where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        var result = self.copy()
        result.update(other)
        return result^

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

    # Append an entry known not to be present, growing the index at load
    # factor one.
    def _append_new(mut self, var entry: DictEntry[Self.K, Self.V]):
        var entry_index = len(self.entries)
        var bucket = bucket_index(entry.key, self.nbuckets)
        self.entries.append(entry^)
        var bucket_entries = self.index._get_copy(bucket)
        bucket_entries.append(entry_index)
        self.index[bucket] = bucket_entries^
        if len(self.entries) == self.nbuckets:
            self._rehash(self.nbuckets * 2)

    # Rebuild the bucket index from the dense entries at the given width.
    def _rehash(mut self, new_bucket_count: Int):
        var new_index = List[List[Int]]()
        var i = 0
        while i < new_bucket_count:
            new_index.append(List[Int]())
            i += 1
        i = 0
        while i < len(self.entries):
            var bucket = bucket_index(
                self.entries._get_copy(i).key, new_bucket_count
            )
            var bucket_entries = new_index._get_copy(bucket)
            bucket_entries.append(i)
            new_index[bucket] = bucket_entries^
            i += 1
        self.index = new_index
        self.nbuckets = new_bucket_count

    # Rebuild the bucket index after a positional removal shifted entries.
    def _reindex(mut self):
        self._rehash(self.nbuckets)

    # Restore the empty eight-bucket index after a drain.
    def _reset_index(mut self):
        self.index = List[List[Int]]()
        self.nbuckets = 8
        var i = 0
        while i < self.nbuckets:
            self.index.append(List[Int]())
            i += 1
