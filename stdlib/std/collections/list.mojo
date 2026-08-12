# A self-hosted growable List.  Heap slots in `[0, size)` are initialized;
# slots in `[size, cap)` are not.  `unsafe_offset(i).unsafe_take_pointee()`
# moves an initialized slot out and `unsafe_offset(i).unsafe_deinit_pointee()`
# destroys one in place — the public raw-pointer vocabulary over storage this
# List owns.

from std.memory import unsafe_alloc

from std.iterable import Iterable, IterableOwned, Iterator, StopIteration

@fieldwise_init
struct _ListIter[
    iterable_mut: Bool, //, T: Movable, iterable_origin: Origin[mut=iterable_mut]
](Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T

    var src: ref[iterable_origin] List[Self.T]
    var index: Int

    # Kept as an optimization hint and compatibility API.  Exhaustion is
    # reported by StopIteration, not by the old HasNext/length sentinel.
    def __len__(self) -> Int:
        return len(self.src) - self.index

    def __next__(mut self) raises StopIteration -> ref[
        iterable_origin._get_owned_interior["element"]
    ] Self.T where conforms_to(Self.T, Copyable):
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]

struct _ListOwnedIter[T: Movable](
    Deinitable where conforms_to(T, Deinitable),
    Iterator,
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

    def _finish(deinit self):
        # Named destructor for the linear-element instantiation, which has no
        # `__deinit__`: the compiler calls it on the loop's exhaustion edge, when
        # every element has been moved out, so only the buffer remains.
        self.data.unsafe_free()

struct List[T: Movable](
    Copyable where conforms_to(T, Copyable),
    Deinitable where conforms_to(T, Deinitable),
    Iterable where conforms_to(T, Copyable),
    IterableOwned,
    Movable,
    Writable where conforms_to(T, Writable),
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _ListIter[Self.T]
    comptime IteratorOwnedType = _ListOwnedIter[Self.T]

    var data: UnsafePointer[Self.T]
    var size: Int
    var cap: Int

    def __init__(out self):
        self.cap = 4
        self.size = 0
        self.data = unsafe_alloc[Self.T](self.cap)

    def __init__(out self, var *values: Self.T, __list_literal__: NoneType):
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
            self.data[i] = copy.data[i]
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

    def grow(mut self):
        var new_cap = self.cap * 2
        var new_data = unsafe_alloc[Self.T](new_cap)
        var i = 0
        while i < self.size:
            new_data[i] = self.data.unsafe_offset(i).unsafe_take_pointee()
            i += 1
        self.data.unsafe_free()
        self.data = new_data
        self.cap = new_cap

    def append(mut self, var value: Self.T):
        if self.size == self.cap:
            self.grow()
        self.data[self.size] = value^
        self.size += 1

    def insert(mut self, index: Int, var value: Self.T):
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
        return self.data[index]

    def __getitem__(self, slice: Slice) -> Self where conforms_to(
        Self.T, Copyable
    ):
        var bounds = slice.indices(self.size)
        var start = bounds[0]
        var stop = bounds[1]
        var step = bounds[2]
        var result = List[Self.T]()
        if step > 0:
            while start < stop:
                result.append(self.data[start])
                start += step
        else:
            while start > stop:
                result.append(self.data[start])
                start += step
        return result^

    def __setitem__(mut self, index: Int, var value: Self.T) where conforms_to(
        Self.T, Deinitable
    ):
        self.data.unsafe_offset(index).unsafe_deinit_pointee()
        self.data[index] = value^

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
    ) and conforms_to(Self.T, Deinitable):
        var i = 0
        while i < self.size:
            if self.data[i] == value:
                var removed = self.pop(i)
                return
            i += 1

    def pop(mut self) -> Self.T:
        return self.pop(self.size - 1)^

    def pop(mut self, index: Int) -> Self.T:
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

    def reverse(mut self):
        var left = 0
        var right = self.size - 1
        while left < right:
            var left_value = self.data.unsafe_offset(left).unsafe_take_pointee()
            var right_value = self.data.unsafe_offset(right).unsafe_take_pointee()
            self.data[left] = right_value^
            self.data[right] = left_value^
            left += 1
            right -= 1

    def extend(mut self, other: Self) where conforms_to(Self.T, Copyable):
        var i = 0
        while i < len(other):
            self.append(other[i])
            i += 1

    def count(self, value: Self.T) -> Int where conforms_to(Self.T, Equatable):
        var result = 0
        var i = 0
        while i < self.size:
            if self.data[i] == value:
                result += 1
            i += 1
        return result

    def index(self, value: Self.T) -> Int where conforms_to(Self.T, Equatable):
        var i = 0
        while i < self.size:
            if self.data[i] == value:
                return i
            i += 1
        return -1

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)] where conforms_to(
        Self.T, Copyable
    ):
        ref source = self
        return _ListIter[Self.T](source, 0)

    def __iter__(var self) -> _ListOwnedIter[Self.T]:
        var result = _ListOwnedIter[Self.T](self.data, self.size)
        self.data = unsafe_alloc[Self.T](0)
        self.size = 0
        self.cap = 0
        return result^

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
