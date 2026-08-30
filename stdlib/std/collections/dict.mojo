# A hash-backed, insertion-ordered dictionary.
#
# Dense entries preserve insertion order; a nested-list index maps each hash
# bucket to entry positions, doubling when the load factor reaches one. The
# iterators borrow the entries list and yield references at `element`
# interior granularity, so mutation during iteration is rejected;
# `keys`/`values`/`items` return self-iterable, non-indexable borrowing
# views without `len`, matching upstream's view surface (value/entry yields
# are read-only — a conservative subset of upstream's mut-following value
# references).

from std.collections.list import List
from std.hashing import bucket_index
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

# The `items` borrowing view: yields whole-entry references at `element`
# interior granularity, read-only.
@fieldwise_init
struct _DictEntryIter[
    iterable_mut: Bool, //,
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
    iterable_origin: Origin[mut=iterable_mut],
](Copyable, Iterator):
    comptime Element = DictEntry[Self.K, Self.V]
    comptime IteratorType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = _DictEntryIter[Self.K, Self.V, view_origin]

    var src: ref[iterable_origin] List[DictEntry[Self.K, Self.V]]
    var index: Int

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> ref[
        Origin[mut=False].cast_from[Self.iterable_origin._get_owned_interior["element"]]
    ] DictEntry[Self.K, Self.V]:
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

# The `keys` borrowing view wraps the entry iterator, as upstream: key
# iteration delegates entry stepping to the wrapped `_DictEntryIter`.
@fieldwise_init
struct _DictKeyIter[
    iterable_mut: Bool, //,
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
    iterable_origin: Origin[mut=iterable_mut],
](Copyable, Iterator):
    comptime Element = Self.K
    comptime IteratorType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = _DictKeyIter[Self.K, Self.V, view_origin]
    comptime dict_entry_iter = _DictEntryIter[Self.K, Self.V, Self.iterable_origin]

    var iter: Self.dict_entry_iter

    # The view iterates itself; a stored view restarts from its own cursor.
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    # Key yields are read-only regardless of the mapping's mutability:
    # writing through a key reference would corrupt the hash invariant
    # (the wrapped entry iterator's yields are already immutable).
    def __next__(mut self) raises StopIteration -> ref[
        self.iter.__next__().key
    ] Self.K:
        return self.iter.__next__().key

# The `values` borrowing view wraps the entry iterator, as upstream. Yields
# are read-only (a conservative subset of upstream's mut-following value
# references).
@fieldwise_init
struct _DictValueIter[
    iterable_mut: Bool, //,
    K: Equatable & Copyable & Movable,
    V: Copyable & Movable,
    iterable_origin: Origin[mut=iterable_mut],
](Copyable, Iterator):
    comptime Element = Self.V
    comptime IteratorType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = _DictValueIter[Self.K, Self.V, view_origin]

    var iter: _DictEntryIter[Self.K, Self.V, Self.iterable_origin]

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    # Value yields follow the entry iterator's capability (read-only here, a
    # conservative subset of upstream's mut-following value references).
    def __next__(mut self) raises StopIteration -> ref[
        self.iter.__next__().value
    ] Self.V:
        return self.iter.__next__().value

# The `take_items` draining iterator: borrows the dictionary mutably and
# moves entries out one at a time, so `len` observably decreases as the
# drain progresses and the dictionary is empty and reusable once it is
# exhausted.
@fieldwise_init
struct _TakeDictEntryIter[
    K: Hashable & Equatable & Copyable & Movable,
    V: Copyable & Movable,
    origin: Origin[mut=True],
](Copyable, Iterator):
    comptime Element = DictEntry[Self.K, Self.V]
    comptime IteratorType[
        view_mut: Bool, //, view_origin: Origin[mut=view_mut]
    ] = _TakeDictEntryIter[Self.K, Self.V, view_origin]

    var src: ref[origin] Dict[Self.K, Self.V]

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> DictEntry[Self.K, Self.V]:
        if len(self.src.entries) == 0:
            raise StopIteration()
        var entry = self.src.entries.pop(0)
        self.src._reindex()
        return entry^

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
    ] = _DictKeyIter[Self.K, Self.V, iterable_origin]
    comptime ValuesIterType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _DictValueIter[Self.K, Self.V, iterable_origin]
    comptime ItemsIterType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _DictEntryIter[Self.K, Self.V, iterable_origin]
    comptime TakeIterType[
        take_origin: Origin[mut=True]
    ] = _TakeDictEntryIter[Self.K, Self.V, take_origin]
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

    # Drain every entry through a lazily draining borrowed iterator: each
    # step moves the next entry out (`len` shrinks as the drain progresses)
    # and the dictionary is empty and reusable once it is exhausted.
    def take_items(mut self) -> Self.TakeIterType[origin_of(self)] where conforms_to(
        Self.K, Deinitable
    ) and conforms_to(Self.V, Deinitable):
        ref source = self
        return _TakeDictEntryIter[Self.K, Self.V](source)

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

    # Borrowing views: each returns a self-iterable, non-indexable iterator
    # that borrows the entries list, so source mutation while a view lives
    # is rejected and no elements are copied at call time.
    def keys(ref self) -> Self.IteratorType[origin_of(self)]:
        ref source = self.entries
        return _DictKeyIter(_DictEntryIter(source, 0))

    def values(ref self) -> Self.ValuesIterType[origin_of(self)]:
        ref source = self.entries
        return _DictValueIter(_DictEntryIter(source, 0))

    def items(ref self) -> Self.ItemsIterType[origin_of(self)]:
        ref source = self.entries
        return _DictEntryIter[Self.K, Self.V](source, 0)

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        ref source = self.entries
        return _DictKeyIter(_DictEntryIter(source, 0))

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
