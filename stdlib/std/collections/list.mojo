# A self-hosted growable List.  Heap slots in `[0, size)` are initialized;
# slots in `[size, cap)` are not.  `unsafe_offset(i).unsafe_take_pointee()`
# moves an initialized slot out and `unsafe_offset(i).unsafe_deinit_pointee()`
# destroys one in place — the public raw-pointer vocabulary over storage this
# List owns.

from std.string import check_slice_bounds

from std.memory import unsafe_alloc

from std.iterable import Iterable, IterableOwned, Iterator, StopIteration

from std.optional import Optional

from std.os import abort

@fieldwise_init
struct _ListIter[
    iterable_mut: Bool, //, T: AnyType, iterable_origin: Origin[mut=iterable_mut]
](Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T

    var src: ref[iterable_origin] List[Self.T]
    var index: Int

    # Kept as an optimization hint and compatibility API.  Exhaustion is
    # reported by StopIteration, not by the old HasNext/length sentinel.
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

struct _ListOwnedIter[T: AnyType](
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

    # The owned iterator iterates itself, so a stored drained-iterator value
    # can sit directly in a `for var … in it^` loop.
    def __iter__(var self) -> Self:
        return self^

    def __next__(mut self) raises StopIteration -> Self.T:
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

struct List[T: AnyType](
    Copyable where conforms_to(T, Copyable),
    Deinitable where conforms_to(T, Deinitable),
    Iterable where conforms_to(T, Copyable),
    IterableOwned where conforms_to(T, Deinitable) and conforms_to(T, Movable),
    Movable,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ListIter[Self.T, iterable_origin]
    comptime IteratorOwnedType = _ListOwnedIter[Self.T]

    var data: UnsafePointer[Self.T]
    var size: Int
    var cap: Int

    def __init__(out self):
        self.cap = 4
        self.size = 0
        self.data = unsafe_alloc[Self.T](self.cap)

    # Capacity hint: the first `capacity` appends never reallocate.
    def __init__(out self, *, capacity: Int):
        self.cap = capacity
        self.size = 0
        self.data = unsafe_alloc[Self.T](self.cap)

    def __init__(out self, *, length: Int, fill: Self.T) where conforms_to(
        Self.T, Copyable
    ):
        self.cap = length
        self.size = length
        self.data = unsafe_alloc[Self.T](self.cap)
        var i = 0
        while i < length:
            self.data.unsafe_offset(i).unsafe_write(copy=fill)
            i += 1

    def __init__(
        out self, var *values: Self.T, __list_literal__: NoneType
    ) where conforms_to(Self.T, Movable):
        self.cap = 4
        self.size = 0
        self.data = unsafe_alloc[Self.T](self.cap)
        for var value in values^:
            self.append(value^)

    def __init__(out self, *, copy: Self) where conforms_to(Self.T, Copyable):
        self.cap = copy.cap
        self.size = copy.size
        self.data = unsafe_alloc[Self.T](copy.cap)
        var i = 0
        while i < copy.size:
            self.data.unsafe_offset(i).unsafe_write(copy=copy.data[i])
            i += 1

    def copy(self) -> Self where conforms_to(Self.T, Copyable):
        return List[Self.T](copy: self)

    def __init__(out self, *, deinit move: Self):
        self.cap = move.cap
        self.size = move.size
        self.data = move.data^

    def __deinit__(deinit self) where conforms_to(Self.T, Deinitable):
        var i = 0
        while i < self.size:
            self.data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self.data.unsafe_free()

    def grow(mut self) where conforms_to(Self.T, Movable):
        var new_cap = self.cap * 2
        if new_cap == 0:
            new_cap = 4
        self._realloc(new_cap)

    # Ensure capacity for at least `capacity` elements; never shrinks.
    def reserve(mut self, capacity: Int) where conforms_to(Self.T, Movable):
        if self.cap >= capacity:
            return
        self._realloc(capacity)

    def append(mut self, var value: Self.T) where conforms_to(Self.T, Movable):
        if self.size == self.cap:
            self.grow()
        self.data[self.size] = value^
        self.size += 1

    def insert(
        mut self, index: Int, var value: Self.T
    ) where conforms_to(Self.T, Movable):
        if self.size == self.cap:
            self.grow()
        var i = self.size
        while i > index:
            self.data[i] = self.data.unsafe_offset(i - 1).unsafe_take_pointee()
            i -= 1
        self.data[index] = value^
        self.size += 1

    def __len__(self) -> Int:
        return self.size

    def __bool__(self) -> Bool:
        return self.size > 0

    def capacity(self) -> Int:
        return self.cap

    # Grow to `length` by appending copies of `fill`, or discard the tail
    # when `length` is smaller.
    def resize(mut self, length: Int, fill: Self.T) where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        if length < self.size:
            self.shrink(length)
            return
        self.reserve(length)
        while self.size < length:
            self.data.unsafe_offset(self.size).unsafe_write(copy=fill)
            self.size += 1

    # Discard elements at the end; aborts when `new_length` exceeds the
    # current length.
    def shrink(mut self, new_length: Int) where conforms_to(Self.T, Deinitable):
        if self.size < new_length:
            abort("shrink: new size is bigger than current")
        var i = new_length
        while i < self.size:
            self.data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self.size = new_length

    def swap_elements(mut self, elt_idx_1: Int, elt_idx_2: Int) where conforms_to(
        Self.T, Movable
    ):
        if elt_idx_1 == elt_idx_2:
            return
        var first = self.data.unsafe_offset(elt_idx_1).unsafe_take_pointee()
        var second = self.data.unsafe_offset(elt_idx_2).unsafe_take_pointee()
        self.data[elt_idx_1] = second^
        self.data[elt_idx_2] = first^

    # A borrowed multi-element pointer over the element storage (current
    # Mojo's `unsafe_ptr` accessor). The interior-generation origin keeps the
    # List alive and stales the pointer when a mutation starts a new
    # generation.
    def unsafe_ptr(ref self) -> Pointer[
        Self.T, origin_of(self)._get_owned_interior["element"]
    ]:
        return self.data.unsafe_origin_cast[
            origin_of(self)._get_owned_interior["element"]
        ]()

    def __getitem__(
        ref self, index: Int
    ) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        return self.data[index]

    # Internal value accessor for generic library code that needs an owned copy.
    # Keeping this distinct from overloaded `__getitem__` also avoids erasing the
    # selected Int subscript contract when the access is nested in a larger place.
    def _get_copy(self, index: Int) -> Self.T where conforms_to(
        Self.T, Copyable
    ):
        return self.data[index].copy()

    def __getitem__(self, slice: StridedSlice) -> Self where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Movable):
        var bounds = slice.indices(self.size)
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
    # or reversed bounds abort instead of normalizing. Strided slicing (and a
    # `Slice`-typed descriptor value, which widens to `StridedSlice` like
    # upstream's implicit conversion) keeps `indices()` normalization through
    # the overload above; omitted bounds default to the full extent.
    def __getitem__(self, slice: ContiguousSlice) -> Self where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Movable):
        var start = slice.start.or_else(0)
        var end = slice.end.or_else(self.size)
        check_slice_bounds(start, end, self.size)
        var result = List[Self.T]()
        var i = start
        while i < end:
            result.append(self._get_copy(i))
            i += 1
        return result^

    def __setitem__(mut self, index: Int, var value: Self.T) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        self.data.unsafe_offset(index).unsafe_deinit_pointee()
        self.data[index] = value^

    # Unchecked element access: out-of-range indices are undefined behavior
    # (the VM still diagnoses them deterministically).
    def unsafe_get(
        ref self, idx: Int
    ) -> ref[origin_of(self)._get_owned_interior["element"]] Self.T:
        return self.data[idx]

    def unsafe_set(mut self, idx: Int, var value: Self.T) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        self.data.unsafe_offset(idx).unsafe_deinit_pointee()
        self.data[idx] = value^

    def __contains__(self, value: Self.T) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        var i = 0
        while i < self.size:
            if self.data[i] == value:
                return True
            i += 1
        return False

    def remove(mut self, value: Self.T) where conforms_to(
        Self.T, Equatable
    ) and conforms_to(Self.T, Deinitable) and conforms_to(Self.T, Movable):
        var i = 0
        while i < self.size:
            if self.data[i] == value:
                var removed = self.pop(i)
                return
            i += 1

    def pop(mut self) -> Self.T where conforms_to(Self.T, Movable):
        return self.pop(self.size - 1)^

    def pop(mut self, index: Int) -> Self.T where conforms_to(Self.T, Movable):
        var result = self.data.unsafe_offset(index).unsafe_take_pointee()
        var i = index
        while i + 1 < self.size:
            self.data[i] = self.data.unsafe_offset(i + 1).unsafe_take_pointee()
            i += 1
        self.size -= 1
        return result^

    def clear(mut self) where conforms_to(Self.T, Deinitable):
        var i = 0
        while i < self.size:
            self.data.unsafe_offset(i).unsafe_deinit_pointee()
            i += 1
        self.size = 0

    # Consuming teardown for any element: each element is handed to the
    # caller-supplied consuming handler (no `Deinitable` requirement), then
    # the buffer is freed. The caller's trailing consumption sees the drained
    # write-back state, so only trivial residual fields remain.
    def deinit_with(deinit self, elt_handler: def(var element: Self.T) capturing[_], /):
        var i = 0
        while i < self.size:
            elt_handler(self.data.unsafe_offset(i).unsafe_take_pointee())
            i += 1
        self.data.unsafe_free()
        self.size = 0
        self.cap = 0

    def reverse(mut self) where conforms_to(Self.T, Movable):
        var left = 0
        var right = self.size - 1
        while left < right:
            var left_value = self.data.unsafe_offset(left).unsafe_take_pointee()
            var right_value = self.data.unsafe_offset(right).unsafe_take_pointee()
            self.data[left] = right_value^
            self.data[right] = left_value^
            left += 1
            right -= 1

    # Consuming extend (upstream's convention): `other`'s elements move in
    # and its storage is released. The `Deinitable` requirement (for the
    # drained husk) is a recorded subset of upstream's Movable-only bound.
    def extend(mut self, var other: Self) where conforms_to(
        Self.T, Deinitable
    ) and conforms_to(Self.T, Movable):
        self.reserve(self.size + other.size)
        var i = 0
        while i < other.size:
            self.append(other.data.unsafe_offset(i).unsafe_take_pointee())
            i += 1
        other.data.unsafe_free()
        other.data = unsafe_alloc[Self.T](0)
        other.size = 0
        other.cap = 0

    # Borrowing extend: copy every element of the view (the source list
    # stays intact).
    def extend(mut self, elements: Span[Self.T, _]) where conforms_to(
        Self.T, Copyable
    ) and conforms_to(Self.T, Movable):
        self.reserve(self.size + len(elements))
        var i = 0
        while i < len(elements):
            self.append(elements[i].copy())
            i += 1

    def count(self, value: Self.T) -> Int where conforms_to(Self.T, Equatable):
        var result = 0
        var i = 0
        while i < self.size:
            if self.data[i] == value:
                result += 1
            i += 1
        return result

    # First index of `value`, or an empty Optional when absent.
    def try_index(self, value: Self.T) -> Optional[Int] where conforms_to(
        Self.T, Equatable
    ):
        var i = 0
        while i < self.size:
            if self.data[i] == value:
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

    def __eq__(self, other: Self, /) -> Bool where conforms_to(
        Self.T, Equatable
    ):
        if self.size != other.size:
            return False
        var i = 0
        while i < self.size:
            if not (self.data[i] == other.data[i]):
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
        if x <= 0 or self.size == 0:
            self.clear()
            return
        var orig = self.copy()
        self.reserve(self.size * x)
        var n = 1
        while n < x:
            self.extend(orig.copy())
            n += 1

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.T, Copyable
    ):
        ref source = self
        return _ListIter[Self.T](source, 0)

    def __iter__(var self) -> _ListOwnedIter[Self.T] where conforms_to(
        Self.T, Deinitable
    ):
        var result = _ListOwnedIter[Self.T](self.data, self.size)
        self.data = unsafe_alloc[Self.T](0)
        self.size = 0
        self.cap = 0
        return result^

    # Move the elements into a fresh allocation of `new_cap` slots.
    def _realloc(mut self, new_cap: Int) where conforms_to(Self.T, Movable):
        var new_data = unsafe_alloc[Self.T](new_cap)
        var i = 0
        while i < self.size:
            new_data[i] = self.data.unsafe_offset(i).unsafe_take_pointee()
            i += 1
        self.data.unsafe_free()
        self.data = new_data
        self.cap = new_cap

    def write_to(self, mut writer: Some[Writer]) where conforms_to(
        Self.T, Writable
    ):
        writer.write("[")
        var i = 0
        while i < self.size:
            if i > 0:
                writer.write(", ")
            writer.write(self.data[i])
            i += 1
        writer.write("]")
