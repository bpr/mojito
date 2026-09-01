# A self-hosted fixed-size Array.  Every slot in `[0, length)` is initialized
# for the whole life of the value; the private `_size` field only diverges from
# `Self.length` when owned iteration moves the storage out, so `__deinit__`
# loops to `_size`, not the comptime length.  `unsafe_offset(i).unsafe_take_pointee()` and
# `unsafe_offset(i).unsafe_deinit_pointee()` are the public raw-pointer
# vocabulary over storage this Array owns.

from std.memory import unsafe_alloc

from std.iterable import Iterable, IterableOwned, Iterator, StopIteration

@fieldwise_init
struct _ArrayIter[
    iterable_mut: Bool, //,
    T: AnyType,
    length: Int,
    iterable_origin: Origin[mut=iterable_mut],
](Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T

    var src: ref[iterable_origin] Array[Self.T, length]
    var index: Int

    def __len__(self) -> Int:
        return len(self.src) - self.index

    def __next__(mut self) raises StopIteration -> ref[
        Self.iterable_origin._get_owned_interior["element"]
    ] Self.T where conforms_to(Self.T, Copyable):
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

struct _ArrayOwnedIter[T: AnyType](
    Deinitable where conforms_to(T, Deinitable),
    Iterator where conforms_to(T, Movable),
    Movable,
):
    comptime Element = Self.T

    var data: UnsafePointer[Self.T]
    var size: Int
    var index: Int

    def __init__(out self, data: UnsafePointer[Self.T], size: Int):
        self.data = data
        self.size = size
        self.index = 0

    def __len__(self) -> Int:
        return self.size - self.index

    def __next__(mut self) raises StopIteration -> Self.T where conforms_to(
        Self.T, Movable
    ):
        if self.index >= self.size:
            raise StopIteration()
        var result = self.data.unsafe_offset(self.index).unsafe_take_pointee()
        self.index += 1
        return result^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        var i = self.index
        while i < self.size:
            self.data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self.data.unsafe_free()

struct Array[T: AnyType, length: Int](
    Comparable where conforms_to(T, Comparable),
    Copyable where conforms_to(T, Copyable),
    Deinitable where conforms_to(T, Deinitable),
    Equatable where conforms_to(T, Equatable),
    Iterable where conforms_to(T, Copyable),
    IterableOwned where conforms_to(T, Movable) and conforms_to(T, Deinitable),
    Movable where conforms_to(T, Movable),
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ArrayIter[Self.T, length, iterable_origin]
    comptime IteratorOwnedType = _ArrayOwnedIter[Self.T]

    var data: UnsafePointer[Self.T]
    var _size: Int

    def __init__(
        out self, var *values: Self.T, __list_literal__: NoneType
    ) where conforms_to(Self.T, Movable):
        self._size = Self.length
        self.data = unsafe_alloc[Self.T](Self.length)
        var i = 0
        for var value in values^:
            self.data[i] = value^
            i += 1

    # Upstream's `Defaultable where conforms_to(T, Defaultable)` conformance
    # (default-constructing every element) needs `Self.T()` — constructing a
    # type parameter's default value in a generic body — which Mojito does
    # not support yet. Recorded as a subset gap on the types.collections
    # parity row.
    def __init__(out self, *, fill: Self.T) where conforms_to(Self.T, Copyable):
        self._size = Self.length
        self.data = unsafe_alloc[Self.T](Self.length)
        var i = 0
        while i < Self.length:
            self.data.unsafe_offset(i).unsafe_write(copy=fill)
            i += 1

    def __init__(out self, *, copy: Self) where conforms_to(Self.T, Copyable):
        self._size = copy._size
        self.data = unsafe_alloc[Self.T](Self.length)
        var i = 0
        while i < copy._size:
            self.data.unsafe_offset(i).unsafe_write(copy=copy.data[i])
            i += 1

    def copy(self) -> Self where conforms_to(Self.T, Copyable):
        return Array(copy: self)

    def __init__(out self, *, deinit move: Self):
        self._size = move._size
        self.data = move.data^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        var i = 0
        while i < self._size:
            self.data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self.data.unsafe_free()

    # Consuming teardown for any element; see `List.deinit_with`.
    def deinit_with(deinit self, elt_handler: def(var element: Self.T) capturing[_], /):
        var i = 0
        while i < self._size:
            elt_handler(self.data.unsafe_offset(i).unsafe_take_pointee())
            i += 1
        self.data.unsafe_free()
        self._size = 0

    def __len__(self) -> Int:
        return Self.length

    def __getitem__(
        ref self, index: Int
    ) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        return self.data[index]

    def __eq__(self, other: Self) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        var i = 0
        while i < Self.length:
            if self.data[i] != other.data[i]:
                return False
            i += 1
        return True

    def __ne__(self, other: Self) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        return not self == other

    # Lexicographic ordering (upstream 2026-08): elements compare pairwise
    # and the first differing pair decides the result.
    def __lt__(self, other: Self) -> Bool where conforms_to(Self.T, Comparable):
        var i = 0
        while i < Self.length:
            if self.data[i] < other.data[i]:
                return True
            if other.data[i] < self.data[i]:
                return False
            i += 1
        return False

    def __le__(self, other: Self) -> Bool where conforms_to(Self.T, Comparable):
        return not (other < self)

    def __gt__(self, other: Self) -> Bool where conforms_to(Self.T, Comparable):
        return other < self

    def __ge__(self, other: Self) -> Bool where conforms_to(Self.T, Comparable):
        return not (self < other)

    def __contains__(self, value: Self.T) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        var i = 0
        while i < Self.length:
            if self.data[i] == value:
                return True
            i += 1
        return False

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.T, Copyable
    ):
        ref source = self
        return _ArrayIter(source, 0)

    def __iter__(var self) -> _ArrayOwnedIter[Self.T] where conforms_to(
        Self.T, Movable
    ) and conforms_to(Self.T, Deinitable):
        var result = _ArrayOwnedIter[Self.T](self.data, self._size)
        self.data = unsafe_alloc[Self.T](0)
        self._size = 0
        return result^

    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.T, Writable
    ):
        writer.write("[")
        var i = 0
        while i < Self.length:
            if i > 0:
                writer.write(", ")
            writer.write(self.data[i])
            i += 1
        writer.write("]")
