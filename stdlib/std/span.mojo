# A borrowed contiguous view over a List's element storage: current Mojo's
# `Span`. The view holds a multi-element origin-bearing pointer plus a
# length. Constructing it from a List lends the List's place to the view (a
# shared loan solved through the `ref [origin]` binding), so the source
# stays alive while any copy of the view does and structural mutation of
# the source conflicts. Element access and the strict contiguous sub-slice
# use current Mojo's strict bounds: violations abort.

from std.collections.list import List

from std.iterable import Iterable, Iterator, StopIteration

from std.os import abort


from std.string import String

# The audited head's strict-slice abort messages with the index and valid
# range interpolated, mirroring `std.string.check_slice_bounds`. This module
# loads before `std.string` (which imports `Span`), so it can reference the
# `String` type but cannot call `std.string`'s defs; the helper is local.
struct _BoundsMessage(Movable, Writer):
    var text: String

    def __init__(out self):
        self.text = String("")

    def write_string(mut self, chunk: String):
        self.text = self.text + chunk


def _check_span_slice_bounds(start: Int, end: Int, length: Int):
    if start < 0 or start > length:
        var message = _BoundsMessage()
        message.write(
            "slice start index ", start, " is out of bounds, valid range is 0 to ", length
        )
        abort(message.text)
    if end < 0 or end > length:
        var message = _BoundsMessage()
        message.write(
            "slice end index ", end, " is out of bounds, valid range is 0 to ", length
        )
        abort(message.text)
    if start > end:
        var message = _BoundsMessage()
        message.write(
            "slice start index ", start, " is greater than slice end index ", end
        )
        abort(message.text)


struct Span[mut: Bool, //, T: Movable, origin: Origin[mut=mut]](
    ImplicitlyCopyable, Iterable where conforms_to(T, Copyable), Movable
):
    comptime Element = Self.T
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = _SpanIter[Self.T, iterable_origin]

    var _data: Pointer[Self.T, Self.origin._get_owned_interior["element"]]
    var _size: Int

    # Current Mojo's pointer-backed construction (`Span(unsafe_ptr=p,
    # length=n)`) over an untracked (raw) pointer: the caller vouches that
    # `length` elements stay live behind it for the origin's lifetime.
    # (Upstream's parameter carries the Span's own origin; Mojito's Pointer
    # parameters bind only exact origins, so the raw spelling is the subset.)
    def __init__(out self, *, unsafe_ptr: Pointer[Self.T, MutUntrackedOrigin], length: Int):
        self._data = unsafe_ptr.unsafe_origin_cast[
            origin._get_owned_interior["element"]
        ]()
        self._size = length

    @implicit
    def __init__(out self, ref [Self.origin] list: List[Self.T]):
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
        Self.origin._get_owned_interior["element"]
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
        _check_span_slice_bounds(start, end, self._size)
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
        Self.iterable_origin._get_owned_interior["element"]
    ] Self.T where conforms_to(Self.T, Copyable):
        if self.index >= len(self.src):
            raise StopIteration()
        var r = self.index
        self.index += 1
        return self.src[r]
