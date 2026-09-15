# The owning, insertion-ordered keyword dictionary used for `**kwargs`.
#
# Current Mojo exposes a String-keyed container instead of materializing keyword
# collectors as a general-purpose hash dictionary. Keeping the key type out of the
# parameter list also makes the call ABI explicit: `StringDict[V]` owns the
# homogeneous values collected at a call boundary. Keys are owned `String`s, as
# upstream's `OwnedKwargsDict.key_type`.

from std.collections.dict import DictEntry, _DictEntryIter, _DictKeyIter
from std.collections.list import List
from std.hashlib import default_hasher, hash
from std.iter import Iterable
from std.collections.optional import Optional

struct StringDict[V: Movable](
    Copyable where conforms_to(V, Copyable), Iterable where conforms_to(V, Copyable)
):
    comptime Element = String
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _DictKeyIter[String, Self.V, default_hasher, iterable_origin]

    var entries: List[DictEntry[String, Self.V]]
    var index: List[List[Int]]
    var nbuckets: Int
    var count: Int

    def __init__(out self):
        self.entries = List[DictEntry[String, Self.V]]()
        self.index = List[List[Int]]()
        self.nbuckets = 8
        self.count = 0
        var i: Int = 0
        while i < self.nbuckets:
            self.index.append(List[Int]())
            i = i + 1

    def __init__(out self, *, copy: Self) where conforms_to(Self.V, Copyable):
        self.entries = List[DictEntry[String, Self.V]](copy: copy.entries)
        self.index = List[List[Int]](copy: copy.index)
        self.nbuckets = copy.nbuckets
        self.count = copy.count

    def copy(self) -> Self where conforms_to(Self.V, Copyable):
        return StringDict[Self.V](copy: self)

    def find_index(self, key: String) -> Int:
        var bucket: Int = Int(hash(key)) & (self.nbuckets - 1)
        for entry_index in self.index._get_copy(bucket):
            ref entry = self.entries[entry_index]
            if entry.key == key:
                return entry_index
        return -1

    def __contains__(self, key: String) -> Bool:
        return self.find_index(key) >= 0

    # A copying read of the value.
    def __getitem__(self, key: String) raises -> Self.V where conforms_to(
        Self.V, Copyable
    ):
        var i: Int = self.find_index(key)
        if i >= 0:
            return self.entries[i].value.copy()
        raise Error("missing key")

    def __setitem__(mut self, key: String, var value: Self.V) where conforms_to(
        Self.V, Deinitable
    ):
        var existing: Int = self.find_index(key)
        if existing >= 0:
            self.entries[existing] = DictEntry[String, Self.V](key, value^)
            return

        var entry_index: Int = len(self.entries)
        self.entries.append(DictEntry[String, Self.V](key, value^))
        var bucket: Int = Int(hash(key)) & (self.nbuckets - 1)
        var bucket_entries: List[Int] = self.index._get_copy(bucket)
        bucket_entries.append(entry_index)
        self.index[bucket] = bucket_entries^
        self.count = self.count + 1
        if self.count == self.nbuckets:
            self.rehash(self.nbuckets * 2)

    def rehash(mut self, new_bucket_count: Int):
        var new_index: List[List[Int]] = List[List[Int]]()
        var i: Int = 0
        while i < new_bucket_count:
            new_index.append(List[Int]())
            i = i + 1
        i = 0
        while i < len(self.entries):
            ref entry = self.entries[i]
            var bucket: Int = Int(entry._hash) & (new_bucket_count - 1)
            var bucket_entries: List[Int] = new_index._get_copy(bucket)
            bucket_entries.append(i)
            new_index[bucket] = bucket_entries^
            i = i + 1
        self.index = new_index^
        self.nbuckets = new_bucket_count

    def bucket_count(self) -> Int:
        return self.nbuckets

    # Displacement-returning insertion: replacing an existing key moves the
    # previous entry (key and value) out and returns it; a fresh key returns
    # an empty Optional. Nothing is destroyed in place, so no `Deinitable`
    # bound is required.
    def insert(mut self, key: String, var value: Self.V) -> Optional[
        DictEntry[String, Self.V]
    ]:
        var existing: Int = self.find_index(key)
        if existing >= 0:
            var displaced = Optional[DictEntry[String, Self.V]](
                self.entries.data.unsafe_offset(existing).unsafe_take_pointee()
            )
            self.entries.data[existing] = DictEntry[String, Self.V](key, value^)
            return displaced^
        # Fresh-key path: append and index directly (like `__setitem__`'s
        # fresh branch) — delegating would demand its `Deinitable` bound.
        var entry_index: Int = len(self.entries)
        self.entries.append(DictEntry[String, Self.V](key, value^))
        var bucket: Int = Int(hash(key)) & (self.nbuckets - 1)
        var bucket_entries: List[Int] = self.index._get_copy(bucket)
        bucket_entries.append(entry_index)
        self.index[bucket] = bucket_entries^
        self.count = self.count + 1
        if self.count == self.nbuckets:
            self.rehash(self.nbuckets * 2)
        return Optional[DictEntry[String, Self.V]]()

    # Consuming teardown: every entry is handed front-to-back to the consuming
    # handler; the bucket index holds only integers and drops with the shell.
    def deinit_with(
        deinit self,
        elt_handler: def(var key: String, var value: Self.V) capturing[_],
        /,
    ):
        while len(self.entries) > 0:
            var entry = self.entries.pop(0)
            entry^.reap_with(elt_handler)

    def get(self, key: String) -> Optional[Self.V] where conforms_to(
        Self.V, Copyable
    ):
        var i: Int = self.find_index(key)
        if i >= 0:
            return Optional[Self.V](self.entries[i].value.copy())
        return Optional[Self.V]()

    def get(self, key: String, default: Self.V) -> Self.V where conforms_to(
        Self.V, Copyable
    ):
        var i: Int = self.find_index(key)
        if i >= 0:
            return self.entries[i].value.copy()
        return default.copy()

    def __len__(self) -> Int:
        return self.count

    def keys(self) -> List[String]:
        var result: List[String] = List[String]()
        var i: Int = 0
        while i < len(self.entries):
            ref entry = self.entries[i]
            result.append(entry.key)
            i = i + 1
        return result^

    def values(self) -> List[Self.V] where conforms_to(Self.V, Copyable):
        var result: List[Self.V] = List[Self.V]()
        var i: Int = 0
        while i < len(self.entries):
            result.append(self.entries[i].value.copy())
            i = i + 1
        return result^

    def items(self) -> List[DictEntry[String, Self.V]] where conforms_to(
        Self.V, Copyable
    ):
        return self.entries.copy()

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.V, Copyable
    ):
        return _DictKeyIter(
            _DictEntryIter(Pointer(to=self.entries).unsafe_origin_cast[origin_of(self)](), 0)
        )
