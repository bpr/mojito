# A borrowed contiguous view over a List's element storage: current Mojo's
# `Span`. The view holds a multi-element origin-bearing pointer plus a
# length. Constructing it from a List lends the List's place to the view (a
# shared loan solved through the `ref [origin]` binding), so the source
# stays alive while any copy of the view does and structural mutation of
# the source conflicts. Element access and the strict contiguous sub-slice
# use current Mojo's strict bounds: violations abort.

from std.string import check_slice_bounds

from std.collections.list import List

from std.iterable import Iterable, Iterator, StopIteration

from std.os import abort


struct Span[mut: Bool, //, T: Movable, origin: Origin[mut=mut]](
    ImplicitlyCopyable, Iterable where conforms_to(T, Copyable), Movable
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _SpanIter[Self.T, iterable_origin]

    var _data: Pointer[Self.T, Self.origin._get_owned_interior["element"]]
    var _size: Int

    @implicit
    def __init__(out self, ref [origin] list: List[Self.T]):
        self._data = list.data.unsafe_origin_cast[
            origin._get_owned_interior["element"]
        ]()
        self._size = len(list)

    def __len__(self) -> Int:
        return self._size

    # Borrowed iteration yields element references like List's (write-through
    # on a mutable source); the iterator borrows the span itself, whose loans
    # keep the underlying List alive.
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        ref source = self
        return _SpanIter[Self.T](source, 0)

    def __getitem__(ref self, index: Int) -> ref[
        origin._get_owned_interior["element"]
    ] Self.T:
        if index < 0 or index >= self._size:
            abort("Span index out of range")
        return self._data[index]

    # Strict contiguous slice (current Mojo bounds): negative, out-of-range,
    # or reversed bounds abort instead of normalizing, and the result is a
    # sub-view of the same storage, not a copy. Spans have no strided
    # slicing.
    def __getitem__(self, slice: ContiguousSlice) -> Self:
        var start = slice.start.or_else(0)
        var end = slice.end.or_else(self._size)
        check_slice_bounds(start, end, self._size)
        var result = self
        result._data = result._data.unsafe_offset(start)
        result._size = end - start
        return result^


# The borrowed Span iterator: borrows its source span (whose loans keep the
# underlying List alive) and yields element references at the span's
# interior-generation granularity, so structural mutation of the source
# during iteration invalidates the yielded references.
@fieldwise_init
struct _SpanIter[
    iterable_mut: Bool, //, T: Movable, iterable_origin: Origin[mut=iterable_mut]
](Iterator where conforms_to(T, Copyable)):
    comptime Element = Self.T

    var src: ref[iterable_origin] Span[Self.T, Self.iterable_origin]
    var index: Int

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
