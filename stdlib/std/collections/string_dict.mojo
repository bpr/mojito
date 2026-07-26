# The owning, insertion-ordered keyword dictionary used for `**kwargs`.
#
# Current Mojo exposes a String-keyed container instead of materializing keyword
# collectors as a general-purpose HashDict. Keeping the key type out of the
# parameter list also makes the call ABI explicit: `StringDict[V]` owns the
# homogeneous values collected at a call boundary.

from std.collections.dict import DictEntry, _DictKeyIter
from std.collections.list import List
from std.hashing import bucket_index
from std.iterable import Iterable
from std.optional import Optional

struct StringDict[V: Copyable & Movable](Copyable, Iterable):
    comptime Element = String
    comptime Iter = _DictKeyIter[String, Self.V]

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

    def __init__(out self, *, copy: Self):
        self.entries = List[DictEntry[String, Self.V]](copy: copy.entries)
        self.index = List[List[Int]](copy: copy.index)
        self.nbuckets = copy.nbuckets
        self.count = copy.count

    def copy(self) -> Self:
        return StringDict[Self.V](copy: self)

    def find_index(self, key: String) -> Int:
        var bucket: Int = bucket_index(key, self.nbuckets)
        for entry_index in self.index._get_copy(bucket):
            if self.entries._get_copy(entry_index).key == key:
                return entry_index
        return -1

    def __contains__(self, key: String) -> Bool:
        return self.find_index(key) >= 0

    def __getitem__(self, key: String) raises -> Self.V:
        var i: Int = self.find_index(key)
        if i >= 0:
            return self.entries._get_copy(i).value
        raise Error("missing key")

    def __setitem__(mut self, key: String, value: Self.V) where conforms_to(
        Self.V, ImplicitlyDeletable
    ):
        var existing: Int = self.find_index(key)
        if existing >= 0:
            self.entries[existing] = DictEntry[String, Self.V](key, value)
            return

        var entry_index: Int = len(self.entries)
        self.entries.append(DictEntry[String, Self.V](key, value))
        var bucket: Int = bucket_index(key, self.nbuckets)
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
            var bucket: Int = bucket_index(
                self.entries._get_copy(i).key, new_bucket_count
            )
            var bucket_entries: List[Int] = new_index._get_copy(bucket)
            bucket_entries.append(i)
            new_index[bucket] = bucket_entries^
            i = i + 1
        self.index = new_index
        self.nbuckets = new_bucket_count

    def bucket_count(self) -> Int:
        return self.nbuckets

    def get(self, key: String) -> Optional[Self.V]:
        var i: Int = self.find_index(key)
        if i >= 0:
            return Optional[Self.V](self.entries._get_copy(i).value, True)
        return Optional[Self.V]()

    def get(self, key: String, default: Self.V) -> Self.V:
        var i: Int = self.find_index(key)
        if i >= 0:
            return self.entries._get_copy(i).value
        return default

    def __len__(self) -> Int:
        return self.count

    def keys(self) -> List[String]:
        var result: List[String] = List[String]()
        var i: Int = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).key)
            i = i + 1
        return result^

    def values(self) -> List[Self.V]:
        var result: List[Self.V] = List[Self.V]()
        var i: Int = 0
        while i < len(self.entries):
            result.append(self.entries._get_copy(i).value)
            i = i + 1
        return result^

    def items(self) -> List[DictEntry[String, Self.V]]:
        return self.entries

    def __iter__(self) -> _DictKeyIter[String, Self.V]:
        return _DictKeyIter[String, Self.V](self.entries)
