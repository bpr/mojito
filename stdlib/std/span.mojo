# A borrowed contiguous view over a List's element storage: current Mojo's
# `Span`. The view holds a multi-element origin-bearing pointer plus a
# length. Constructing it from a List lends the List's place to the view (a
# shared loan solved through the `ref [origin]` binding), so the source
# stays alive while any copy of the view does and structural mutation of
# the source conflicts. Element access and the strict contiguous sub-slice
# use current Mojo's strict bounds: violations abort.

from std.collections.list import List

from std.os import abort


struct Span[mut: Bool, //, T: Movable, origin: Origin[mut=mut]](
    ImplicitlyCopyable, Movable
):
    comptime Element = Self.T

    var _data: Pointer[Self.T, Self.origin._get_owned_interior["element"]]
    var _size: Int

    def __init__(out self, ref [origin] list: List[Self.T]):
        self._data = list.data.origin_cast[
            origin._get_owned_interior["element"]
        ]()
        self._size = len(list)

    def __len__(self) -> Int:
        return self._size

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
        if start < 0 or end > self._size or start > end:
            abort("Span slice bounds out of range")
        var result = self
        result._data = result._data.unsafe_offset(start)
        result._size = end - start
        return result^
