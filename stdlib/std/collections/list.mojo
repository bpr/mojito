# A self-hosted growable List over upstream's two fields: `_storage`, the
# `Allocation` whose `Layout` carries the capacity, and `_len`.  Heap slots in
# `[0, _len)` are initialized — or, after an `unsafe_uninit_length`
# construction or resize, owed an initialization by the caller — and slots in
# `[_len, capacity())` are not.
# `unsafe_offset(i).unsafe_take_pointee()`
# moves an initialized slot out and `unsafe_offset(i).unsafe_deinit_pointee()`
# destroys one in place — the public raw-pointer vocabulary over storage this
# List owns.

from std.string import check_slice_bounds
from std.builtin.builtin_slice import ContiguousSlice, StridedSlice
from std.builtin.int import Indexer, index
from std.span import Span

from std.memory.alloc import Allocation, Layout, alloc, dealloc

from std.reflection.type_info import _unqualified_type_name
from std.hash import Hasher
from std.range import range
from std.iter import Iterable, IterableOwned, Iterator, StopIteration

from std.collections.optional import Optional


@fieldwise_init
struct _ListIter[
    iterable_mut: Bool, //, T: AnyType, iterable_origin: Origin[mut=iterable_mut]
](Copyable, Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self

    var src: ref[iterable_origin] List[Self.T]
    var index: Int

    # Kept as an optimization hint and compatibility API.  Exhaustion is
    # reported by StopIteration, not by the old HasNext/length sentinel.
    def __len__(self) -> Int:
        return len(self.src) - self.index

    # The iterator iterates itself (upstream's `IteratorType = Self`): a
    # stored iterator drives a loop directly.
    @inline(.always)
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> ref[
        Self.iterable_origin._get_owned_interior["element"]
    ] Self.T where conforms_to(Self.T, Copyable):
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

# The borrowed back-to-front iterator `reversed(list)` returns: `_ListIter`'s
# shape with the index counting down. It iterates itself so a temporary
# (`for x in reversed(xs)`) drives a loop directly.
@fieldwise_init
struct _ListReversedIter[
    iterable_mut: Bool, //, T: AnyType, iterable_origin: Origin[mut=iterable_mut]
](Copyable, Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self

    var src: ref[iterable_origin] List[Self.T]
    var remaining: Int

    def __len__(self) -> Int:
        return self.remaining

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> ref[
        Self.iterable_origin._get_owned_interior["element"]
    ] Self.T where conforms_to(Self.T, Copyable):
        if self.remaining <= 0:
            raise StopIteration()
        self.remaining -= 1
        return self.src[self.remaining]

# The consuming iterator `List.__iter__(var self)` returns: it holds the
# drained `List` itself and moves elements out of its buffer by index. Its
# destructor destroys the untaken tail and zeroes `_list._len`, so the held
# `List` then frees only its buffer.
@fieldwise_init
struct _ListIterOwned[T: Movable & Deinitable](
    IterableOwned, Iterator, Movable
):
    comptime Element = Self.T
    comptime IteratorOwnedType = Self

    var _list: List[Self.T]
    var _index: Int

    @inline(.always)
    def __deinit__(deinit self):
        var data = self._list._storage._alloc._ptr
        var i = self._index
        while i < len(self._list):
            data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self._list._len = 0

    @inline(.always)
    def __iter__(var self) -> Self.IteratorOwnedType:
        return self^

    def __next__(mut self) raises StopIteration -> Self.Element:
        if self._index >= len(self._list):
            raise StopIteration()
        self._index += 1
        return self._list._storage._alloc._ptr.unsafe_offset(
            self._index - 1
        ).unsafe_take_pointee()

    def __len__(self) -> Int:
        return len(self._list) - self._index

@explicit_destroy(
    "Use `deinit_with()` to explicitly destroy a `List` of"
    " non-`Deinitable` elements"
)
struct List[T: AnyType](
    Boolable,
    Copyable where conforms_to(T, Copyable),
    Defaultable,
    Deinitable where conforms_to(T, Deinitable),
    Equatable where conforms_to(T, Equatable),
    Hashable where conforms_to(T, Hashable),
    Iterable where conforms_to(T, Copyable),
    IterableOwned where conforms_to(T, Deinitable & Movable),
    Movable,
    Sized,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ListIter[Self.T, iterable_origin]
    comptime IteratorOwnedType: Iterator where conforms_to(
        Self.T, Deinitable & Movable
    ) = _ListIterOwned[Self.T]
    comptime ReversedIterType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ListReversedIter[Self.T, iterable_origin]

    # The storage backing the list; its `Layout` carries the capacity.
    var _storage: Allocation[Self.T]
    var _len: Int

    def __init__(out self):
        self._storage = alloc(Layout[Self.T](count=4))
        self._len = 0

    # Capacity hint: the first `capacity` appends never reallocate.
    def __init__(out self, *, capacity: Int):
        self._storage = alloc(Layout[Self.T](count=capacity))
        self._len = 0

    # Length without initialization: the caller writes every slot in
    # `[0, length)` through raw pointer stores before it is read, copied, or
    # destroyed. The VM keeps a never-written slot distinct from a taken one
    # (reads trap, a take yields a tombstone, a destroy is a no-op); native
    # storage is raw bytes, as upstream.
    @inline(.always)
    def __init__(out self, *, unsafe_uninit_length: Int):
        self._storage = alloc(Layout[Self.T](count=unsafe_uninit_length))
        self._len = unsafe_uninit_length

    def __init__(out self, *, length: Int, fill: Self.T) where conforms_to(
        Self.T, Copyable
    ):
        self._storage = alloc(Layout[Self.T](count=length))
        self._len = length
        var i = 0
        while i < length:
            self._storage._alloc._ptr.unsafe_offset(i).unsafe_write(copy=fill)
            i += 1

    @inline(.always)
    def __init__(
        out self, var *values: Self.T, __list_literal__: NoneType
    ) where conforms_to(Self.T, Movable):
        self._storage = alloc(Layout[Self.T](count=4))
        self._len = 0
        for var value in values^:
            self.append(value^)

    # Upstream's clause, `where conforms_to(IterableType.IteratorType[
    # origin_of(iterable)].Element, Copyable)`, takes a projection subject
    # (R792). Its body sizes the list by `iter(iterable).bounds()` (R670,
    # R669) and moves each element in with `rebind_var` from a loop that
    # yields the iterator's element (R178); this one copies the iterable's
    # `Element` and rebinds it.
    def __init__[
        IterableType: Iterable,
    ](
        ref iterable: IterableType,
        out self: List[IterableType.IteratorType[origin_of(iterable)].Element],
    ):
        self = type_of(self)()
        for value in iterable:
            var element = value.copy()
            self.append(rebind[type_of(self).T](element)^)

    def __init__(out self, *, copy: Self) where conforms_to(Self.T, Copyable):
        self._storage = alloc(Layout[Self.T](count=copy.capacity()))
        self._len = copy._len
        var data = self._storage._alloc._ptr
        var source = copy._storage._alloc._ptr
        var i = 0
        while i < copy._len:
            data.unsafe_offset(i).unsafe_write(copy=source[i])
            i += 1

    def copy(self) -> Self where conforms_to(Self.T, Copyable):
        return List[Self.T](copy: self)

    def __init__(out self, *, deinit move: Self):
        self._storage = move._storage^
        self._len = move._len

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        var i = 0
        while i < self._len:
            self._storage._alloc._ptr.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        dealloc(self._storage^)

    def grow(mut self) where conforms_to(Self.T, Movable):
        var new_cap = self.capacity() * 2
        if new_cap == 0:
            new_cap = 4
        self._realloc(new_cap)

    # Ensure capacity for at least `capacity` elements; never shrinks.
    def reserve(mut self, capacity: Int) where conforms_to(Self.T, Movable):
        if self.capacity() >= capacity:
            return
        self._realloc(capacity)

    @__unsafe_nested_origins_read_only
    def append(mut self, var value: Self.T) where conforms_to(Self.T, Movable):
        if self._len == self.capacity():
            self.grow()
        self._storage._alloc._ptr[self._len] = value^
        self._len += 1

    @inline(.always)
    def insert(
        mut self, index: Int, var value: Self.T
    ) where conforms_to(Self.T, Movable):
        if self._len == self.capacity():
            self.grow()
        var data = self._storage._alloc._ptr
        var i = self._len
        while i > index:
            data[i] = data.unsafe_offset(i - 1).unsafe_take_pointee()
            i -= 1
        data[index] = value^
        self._len += 1

    @inline(.nodebug)
    def __len__(self) -> Int:
        return self._len

    def __bool__(self) -> Bool:
        return self._len > 0

    @inline(.nodebug)
    def capacity(self) -> Int:
        return len(self._storage)

    # Grow to `length` by appending copies of `fill`, or discard the tail
    # when `length` is smaller.
    def resize(mut self, length: Int, fill: Self.T) where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        if length < self._len:
            self.shrink(length)
            return
        self.reserve(length)
        while self._len < length:
            self._storage._alloc._ptr.unsafe_offset(self._len).unsafe_write(copy=fill)
            self._len += 1

    # Grow to `unsafe_uninit_length` leaving the new slots uninitialized (the
    # caller's raw-pointer stores initialize them), or discard the tail.
    def resize(mut self, *, unsafe_uninit_length: Int) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        if unsafe_uninit_length <= self._len:
            self.shrink(unsafe_uninit_length)
            return
        self.reserve(unsafe_uninit_length)
        self._len = unsafe_uninit_length

    # Discard elements at the end; aborts when `new_length` exceeds the
    # current length.
    def shrink(mut self, new_length: Int) where conforms_to(Self.T, Deinitable):
        if self._len < new_length:
            _mojito_abort("shrink: new size is bigger than current")
        var i = new_length
        while i < self._len:
            self._storage._alloc._ptr.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self._len = new_length

    def swap_elements(mut self, elt_idx_1: Int, elt_idx_2: Int) where conforms_to(
        Self.T, Movable
    ):
        if elt_idx_1 == elt_idx_2:
            return
        var data = self._storage._alloc._ptr
        var first = data.unsafe_offset(elt_idx_1).unsafe_take_pointee()
        var second = data.unsafe_offset(elt_idx_2).unsafe_take_pointee()
        data[elt_idx_1] = second^
        data[elt_idx_2] = first^

    # A borrowed multi-element pointer over the element storage (current
    # Mojo's `unsafe_ptr` accessor). The interior-generation origin keeps the
    # List alive and stales the pointer when a mutation starts a new
    # generation.
    def unsafe_ptr(ref self) -> Pointer[
        Self.T, origin_of(self)._get_owned_interior["element"]
    ]:
        return self._storage._alloc._ptr.unsafe_origin_cast[
            origin_of(self)._get_owned_interior["element"]
        ]()

    # Strict bounds, as `Span.__getitem__` has. The pinned Mojo aborts here
    # with an `Assert Error` at every optimization level, and the VM's arena
    # catches the raw read, but the native backend has no arena: unchecked,
    # `xs[10]` on a three-element list read out of bounds and returned a value.
    @__unsafe_nested_origins_read_only
    @inline(.always)
    def __getitem__(
        ref self, index: Int
    ) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        if index < 0 or index >= self._len:
            _mojito_abort("List index out of range")
        return self._storage._alloc._ptr[index]

    @__unsafe_nested_origins_read_only
    @inline(.always)
    def __getitem__(
        ref self, idx: Some[Indexer]
    ) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        var i = index(idx)
        if i < 0 or i >= self._len:
            _mojito_abort("List index out of range")
        return self._storage._alloc._ptr[i]

    # Internal value accessor for generic library code that needs an owned copy.
    # Keeping this distinct from overloaded `__getitem__` also avoids erasing the
    # selected Int subscript contract when the access is nested in a larger place.
    def _get_copy(self, index: Int) -> Self.T where conforms_to(
        Self.T, Copyable
    ):
        return self._storage._alloc._ptr[index].copy()

    def __getitem__(self, slice: StridedSlice) -> Self where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Movable):
        var bounds = slice.indices(self._len)
        var start = bounds[0]
        var stop = bounds[1]
        var step = bounds[2]
        var result = List[Self.T]()
        if step > 0:
            while start < stop:
                result.append(self._get_copy(start))
                start += step
        else:
            while start > stop:
                result.append(self._get_copy(start))
                start += step
        return result^

    # Strict contiguous slice (current Mojo bounds): negative, out-of-range,
    # or reversed bounds abort instead of normalizing, and the result is a
    # sub-view borrowing the list's owned elements, not a copy. Strided
    # slicing (and a `Slice` value, which converts to `StridedSlice` through
    # its `@implicit` constructor) keeps `indices()` normalization and copies
    # through the overload above; omitted bounds default to the full extent.
    @__unsafe_nested_origins_read_only
    @inline(.always)
    def __getitem__(
        ref self, slice: ContiguousSlice
    ) -> Span[Self.T, origin_of(self)._get_owned_interior["element"]]:
        var start = slice.start.or_else(0)
        var end = slice.end.or_else(self._len)
        check_slice_bounds(start, end, self._len)
        return {
            unsafe_ptr = self.unsafe_ptr().unsafe_offset(start),
            length = end - start,
        }

    # Unchecked element access: out-of-range indices are undefined behavior
    # (the VM still diagnoses them deterministically).
    @__unsafe_nested_origins_read_only
    @inline(.always)
    def unsafe_get(
        ref self, idx: Int
    ) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        return self._storage._alloc._ptr[idx]

    @inline(.always)
    def unsafe_set(mut self, idx: Int, var value: Self.T) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        self._storage._alloc._ptr.unsafe_offset(idx).unsafe_deinit_pointee()
        self._storage._alloc._ptr[idx] = value^

    # Upstream feeds each element to the caller's hasher with no length
    # prefix (`element.__hash__(hasher)`), read where it lies. The walk
    # indexes through `unsafe_get`: the borrowed `for
    # element in self` loop would need `Copyable` (`_ListIter` is `Iterator
    # where conforms_to(T, Copyable)`), while the conformance is
    # Hashable-only as upstream's.
    def __hash__[H: Hasher](self, mut hasher: H) where conforms_to(Self.T, Hashable):
        for i in range(len(self)):
            self.unsafe_get(i).__hash__(hasher)

    def __contains__(self, value: Self.T) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        var i = 0
        while i < self._len:
            if self._storage._alloc._ptr[i] == value:
                return True
            i += 1
        return False

    def remove(mut self, value: Self.T) where conforms_to(
        Self.T, Equatable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        var i = 0
        while i < self._len:
            if self._storage._alloc._ptr[i] == value:
                var removed = self.pop(i)
                return
            i += 1

    def pop(mut self) -> Self.T where conforms_to(Self.T, Movable):
        return self.pop(self._len - 1)^

    @inline(.always)
    def pop(mut self, index: Int) -> Self.T where conforms_to(Self.T, Movable):
        var data = self._storage._alloc._ptr
        var result = data.unsafe_offset(index).unsafe_take_pointee()
        var i = index
        while i + 1 < self._len:
            data[i] = data.unsafe_offset(i + 1).unsafe_take_pointee()
            i += 1
        self._len -= 1
        return result^

    def clear(mut self) where conforms_to(Self.T, Deinitable):
        var i = 0
        while i < self._len:
            self._storage._alloc._ptr.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self._len = 0

    # Consuming teardown for any element: each element is handed to the
    # caller-supplied consuming handler (no `Deinitable` requirement), then
    # the buffer is freed. The caller's trailing consumption sees the drained
    # write-back state, so only trivial residual fields remain.
    def deinit_with[F: def(var Self.T)](deinit self, deinit_func: F, /):
        var i = 0
        while i < self._len:
            deinit_func(
                self._storage._alloc._ptr.unsafe_offset(i).unsafe_take_pointee()
            )
            i += 1
        dealloc(self._storage^)

    def reverse(mut self) where conforms_to(Self.T, Movable):
        var left = 0
        var right = self._len - 1
        while left < right:
            var data = self._storage._alloc._ptr
            var left_value = data.unsafe_offset(left).unsafe_take_pointee()
            var right_value = data.unsafe_offset(right).unsafe_take_pointee()
            data[left] = right_value^
            data[right] = left_value^
            left += 1
            right -= 1

    # Consuming extend (upstream's convention): `other`'s elements move in
    # and its storage is released. The `Deinitable` requirement (for the
    # drained husk) is a recorded subset of upstream's Movable-only bound.
    def extend(mut self, var other: Self) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        self.reserve(self._len + other._len)
        var source = other._storage._alloc._ptr
        var i = 0
        while i < other._len:
            self.append(source.unsafe_offset(i).unsafe_take_pointee())
            i += 1
        var drained = other._storage^
        other._storage = alloc(Layout[Self.T](count=0))
        other._len = 0
        dealloc(drained^)

    # Borrowing extend: copy every element of the view (the source list
    # stays intact).
    def extend(mut self, elements: Span[Self.T, _]) where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Movable):
        self.reserve(self._len + len(elements))
        var i = 0
        while i < len(elements):
            self.append(elements[i].copy())
            i += 1

    def count(self, value: Self.T) -> Int where conforms_to(Self.T, Equatable):
        var result = 0
        var i = 0
        while i < self._len:
            if self._storage._alloc._ptr[i] == value:
                result += 1
            i += 1
        return result

    # First index of `value`, or an empty Optional when absent.
    def try_index(self, value: Self.T) -> Optional[Int] where conforms_to(
        Self.T, Equatable
    ):
        var i = 0
        while i < self._len:
            if self._storage._alloc._ptr[i] == value:
                return Optional[Int](i)
            i += 1
        return Optional[Int]()

    # First index of `value`; raises upstream's ValueError message when the
    # value is absent.
    def index(self, value: Self.T) raises -> Int where conforms_to(
        Self.T, Equatable
    ):
        var result = self.try_index(value)
        if not Bool(result):
            raise Error("ValueError: Given element is not in list")
        return result.value()

    @inline(.always)
    def __eq__(self, other: Self, /) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        if self._len != other._len:
            return False
        var i = 0
        while i < self._len:
            if not (self._storage._alloc._ptr[i] == other._storage._alloc._ptr[i]):
                return False
            i += 1
        return True

    def __ne__(self, other: Self, /) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        return not (self == other)

    # Concatenation copies self and consumes `other`.
    def __add__(self, var other: Self) -> Self where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        var result = self.copy()
        result.extend(other^)
        return result^

    def __iadd__(mut self, var other: Self, /) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        self.extend(other^)

    # Repetition: `x` copies of the elements (empty for `x <= 0`).
    def __mul__(self, x: Int) -> Self where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        if x <= 0:
            return List[Self.T]()
        var result = self.copy()
        var n = 1
        while n < x:
            result.extend(self.copy())
            n += 1
        return result^

    def __imul__(mut self, x: Int) where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        if x <= 0 or self._len == 0:
            self.clear()
            return
        var orig = self.copy()
        self.reserve(self._len * x)
        var n = 1
        while n < x:
            self.extend(orig.copy())
            n += 1

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.T, Copyable
    ):
        ref source = self
        return _ListIter[Self.T](source, 0)

    def __reversed__(ref self) -> Self.ReversedIterType[origin_of(self)] where conforms_to(
        Self.T, Copyable
    ):
        ref source = self
        return _ListReversedIter[Self.T](source, len(self))

    def __iter__(
        var self,
    ) -> Self.IteratorOwnedType where conforms_to(Self.T, Deinitable & Movable):
        return {self^, 0}

    # Move the elements into a fresh allocation of `new_cap` slots.
    @inline(.never)
    def _realloc(mut self, new_cap: Int) where conforms_to(Self.T, Movable):
        var new_storage = alloc(Layout[Self.T](count=new_cap))
        var new_data = new_storage._alloc._ptr
        var old_data = self._storage._alloc._ptr
        var i = 0
        while i < self._len:
            new_data[i] = old_data.unsafe_offset(i).unsafe_take_pointee()
            i += 1
        var old_storage = self._storage^
        self._storage = new_storage^
        dealloc(old_storage^)

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.T, Writable
    ):
        writer.write("[")
        var i = 0
        while i < self._len:
            if i > 0:
                writer.write(", ")
            writer.write(self._storage._alloc._ptr[i])
            i += 1
        writer.write("]")

    # Upstream's text: `List[SIMD[DType.int, 1]]([Int(1), Int(2)])`.
    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.T, Writable
    ):
        writer.write("List[", _unqualified_type_name[Self.T](), "]([")
        var i = 0
        while i < self._len:
            if i > 0:
                writer.write(", ")
            writer.write(repr(self._storage._alloc._ptr[i]))
            i += 1
        writer.write("])")
