# Upstream's `std/collections/string/iterators.mojo` at its own module
# path: `CodepointSliceIter`, `CodepointsIter`, and `GraphemeSliceIter`,
# each holding the remaining `StringSpan` and shrinking it from the front
# (`next`) or the back (`next_back`), with `forward` choosing which one
# `__next__` takes.
#
# Bodies step through the bundled `StringSpan`'s UTF-8 and segmentation
# helpers where upstream's read `_utf8` and `_grapheme_break`, so
# `GraphemeSliceIter` carries no `_state` or `_state_primed` (R225).
#
# Omitted: `GraphemeIndicesIter` and `BytesIter` (R2084).
"""Implements the iterators and related utilities for efficient string
operations."""

from std.collections.optional import Optional
from std.iter import Iterable, Iterator, StopIteration
from std.string import Codepoint, StringSpan


struct CodepointSliceIter[
    origin: ImmOrigin,
    forward: Bool = True,
](ImplicitlyCopyable, Iterable, Iterator, Sized):
    """Iterator for `StringSlice` over substring slices containing a single
    Unicode codepoint.

    Parameters:
        origin: The origin of the underlying string data.
        forward: The iteration direction. `False` is backwards.

    The `forward` parameter only controls the behavior of the `__next__()`
    method used for normal iteration. Calls to `next()` will always take an
    element from the front of the iterator, and calls to `next_back()` will
    always take an element from the end.
    """

    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator = Self
    """The iterator type for this codepoint iterator.

    Parameters:
        iterable_mut: Whether the iterable is mutable.
        iterable_origin: The origin of the iterable.
    """

    comptime Element = StringSpan[Self.origin]
    """The element type yielded by iteration."""

    var _slice: StringSpan[Self.origin]

    @doc_hidden
    def __init__(out self, str_slice: StringSpan[Self.origin]):
        self._slice = str_slice

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        """Iterate over the `StringSlice` yielding individual characters.

        Returns:
            An iterator over the characters in the string slice.
        """
        return self.copy()

    @inline(.always)
    def __next__(mut self) raises StopIteration -> StringSpan[Self.origin]:
        """Get the next codepoint in the underlying string slice.

        If `forward` is set to `False`, this will return the next codepoint
        from the end of the string.

        Returns:
            The next character in the string.

        Raises:
            `StopIteration` if the iterator has been exhausted.
        """
        if self._slice.byte_length() <= 0:
            raise StopIteration()

        comptime if Self.forward:
            return self.next().value()
        else:
            return self.next_back().value()

    @inline(.always)
    def __len__(self) -> Int:
        """Returns the remaining length of this iterator in `Codepoint`s.

        Returns:
            Number of codepoints remaining in this iterator.
        """
        return self._slice.count_codepoints()

    def peek_next(self) -> Optional[StringSpan[Self.origin]]:
        """Check what the next single-codepoint slice in this iterator is,
        without advancing the iterator state.

        Returns:
            The next codepoint slice in the underlying string, or None if the
            string is empty.
        """
        if self._slice.byte_length() > 0:
            return self._slice._sub_view(0, self._slice._width_at(0))
        else:
            return None

    def peek_back(mut self) -> Optional[StringSpan[Self.origin]]:
        """Check what the last single-codepoint slice in this iterator is,
        without advancing the iterator state.

        Returns:
            The last codepoint slice in the underlying string, or None if the
            string is empty.
        """
        var total = self._slice.byte_length()
        if total > 0:
            var start = total - 1
            while start > 0 and self._slice._is_continuation(
                Int(self._slice._data[start])
            ):
                start -= 1
            return self._slice._sub_view(start, total)
        else:
            return None

    def next(mut self) -> Optional[StringSpan[Self.origin]]:
        """Get the next codepoint slice in the underlying string slice, or None
        if the iterator is empty.

        Returns:
            A character if the string is not empty, otherwise None.
        """
        if self._slice.byte_length() <= 0:
            return None
        var slice_len = self._slice._width_at(0)
        var result = self._slice
        result._size = slice_len
        self._slice._data = self._slice._data.unsafe_offset(slice_len)
        self._slice._size -= slice_len
        return result

    def next_back(mut self) -> Optional[StringSpan[Self.origin]]:
        """Get the last single-codepoint slice in this iterator is, or None
        if the iterator is empty.

        Returns:
            The last codepoint slice in the underlying string, or None if the
            string is empty.
        """
        var total = self._slice.byte_length()
        if total <= 0:
            return None
        var start = total - 1
        while start > 0 and self._slice._is_continuation(
            Int(self._slice._data[start])
        ):
            start -= 1
        var result = self._slice
        result._data = result._data.unsafe_offset(start)
        result._size = total - start
        self._slice._size = start
        return result


struct CodepointsIter[origin: ImmOrigin](
    ImplicitlyCopyable, Iterable, Iterator, Sized
):
    """Iterator over the `Codepoint`s in a string slice, constructed by
    `StringSlice.codepoints()`.

    Parameters:
        origin: Origin of the underlying string data.
    """

    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator = Self
    """The iterator type for this codepoint iterator.

    Parameters:
        iterable_mut: Whether the iterable is mutable.
        iterable_origin: The origin of the iterable.
    """

    comptime Element = Codepoint
    """The element type yielded by iteration."""

    var _slice: StringSpan[Self.origin]
    """String slice containing the bytes that have not been read yet."""

    @doc_hidden
    def __init__(out self, str_slice: StringSpan[Self.origin]):
        self._slice = str_slice

    @doc_hidden
    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self.copy()

    def __next__(mut self) raises StopIteration -> Codepoint:
        """Get the next codepoint in the underlying string slice.

        Returns:
            The next character in the string.

        Raises:
            StopIteration: If the iterator is exhausted.
        """
        if self._slice.byte_length() <= 0:
            raise StopIteration()
        return self.next().value()

    @inline(.always)
    def __len__(self) -> Int:
        """Returns the remaining length of this iterator in `Codepoint`s.

        Returns:
            Number of codepoints remaining in this iterator.
        """
        return self._slice.count_codepoints()

    def peek_next(self) -> Optional[Codepoint]:
        """Check what the next codepoint in this iterator is, without advancing
        the iterator state.

        Returns:
            The next character in the underlying string, or None if the string
            is empty.
        """
        if self._slice.byte_length() > 0:
            var width = self._slice._width_at(0)
            var text = self._slice._with_bytes(0, width)
            return Codepoint(self._slice._scalar_at(0, width), text: text^)
        else:
            return None

    def next(mut self) -> Optional[Codepoint]:
        """Get the next codepoint in the underlying string slice, or None if
        the iterator is empty.

        Returns:
            A character if the string is not empty, otherwise None.
        """
        var result: Optional[Codepoint] = self.peek_next()
        if result:
            var char_len = self._slice._width_at(0)
            self._slice._data = self._slice._data.unsafe_offset(char_len)
            self._slice._size -= char_len
        return result


struct GraphemeSliceIter[
    origin: ImmOrigin,
    forward: Bool = True,
](ImplicitlyCopyable, Iterable, Iterator, Sized):
    """Iterator over grapheme clusters in a string, yielding each cluster as a
    `StringSlice`.

    Parameters:
        origin: The origin of the underlying string data.
        forward: The iteration direction. `False` is backwards.

    The `forward` parameter only controls the behavior of the `__next__()`
    method used for normal iteration. Calls to `next()` will always take an
    element from the front of the iterator, and calls to `next_back()` will
    always take an element from the end. A forward `next()` advances the
    front and invalidates the cached reverse safe-start so the next reverse
    call recomputes it.
    """

    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ]: Iterator = Self
    """The iterator type.

    Parameters:
        iterable_mut: Whether the iterable is mutable.
        iterable_origin: The origin of the iterable.
    """

    comptime Element = StringSpan[Self.origin]
    """The element type yielded by iteration."""

    var _slice: StringSpan[Self.origin]
    """Remaining bytes to iterate over."""

    var _back_safe_known: Bool
    """True if `_back_safe_start` holds a usable safe boundary for reverse
    iteration."""

    var _back_safe_start: Int
    """Cached byte offset in `_slice` at which a fresh forward UAX #29 scan
    can restart and reproduce the same boundaries it would from offset 0."""

    @doc_hidden
    def __init__(out self, str_slice: StringSpan[Self.origin]):
        """Construct from a string slice.

        Args:
            str_slice: The string slice to iterate over.
        """
        self._slice = str_slice
        self._back_safe_known = False
        self._back_safe_start = 0

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        """Return an iterator over grapheme clusters.

        Returns:
            A copy of this iterator.
        """
        return self.copy()

    @inline(.always)
    def __next__(mut self) raises StopIteration -> StringSpan[Self.origin]:
        """Get the next grapheme cluster.

        If `forward` is set to `False`, this will return the next grapheme
        cluster from the end of the string.

        Returns:
            The next grapheme cluster as a `StringSlice`.

        Raises:
            `StopIteration` if the iterator has been exhausted.
        """
        if self._slice.byte_length() <= 0:
            raise StopIteration()
        comptime if Self.forward:
            return self.next().value()
        else:
            return self.next_back().value()

    def __len__(self) -> Int:
        """Return the number of remaining grapheme clusters.

        Returns:
            The number of grapheme clusters remaining.
        """
        return self._slice.count_graphemes()

    @inline(.always)
    def remaining_byte_length(self) -> Int:
        """Returns the number of bytes not yet consumed by the iterator.

        Returns:
            The byte length of the iterator's remaining range.
        """
        return self._slice.byte_length()

    def next(mut self) -> Optional[StringSpan[Self.origin]]:
        """Get the next grapheme cluster, or `None` if exhausted.

        Returns:
            The next grapheme cluster as a `StringSlice`, or `None`.
        """
        var total = self._slice.byte_length()
        if total <= 0:
            return None
        var consumed = self._slice._next_grapheme_end(0)
        var result = self._slice
        result._size = consumed
        self._slice._data = self._slice._data.unsafe_offset(consumed)
        self._slice._size = total - consumed
        self._back_safe_known = False
        return result

    def peek_back(mut self) -> Optional[StringSpan[Self.origin]]:
        """Return the last grapheme cluster without advancing the iterator.

        Returns:
            The last grapheme cluster as a `StringSlice`, or `None` if the
            iterator is empty.
        """
        var total = self._slice.byte_length()
        if total <= 0:
            return None
        var grapheme_start = self._grapheme_start_of_last_cluster(total)
        return self._slice._sub_view(grapheme_start, total)

    def next_back(mut self) -> Optional[StringSpan[Self.origin]]:
        """Get the last grapheme cluster in the underlying string, or `None`
        if the iterator is empty.

        Returns:
            The last grapheme cluster as a `StringSlice`, or `None`.
        """
        var total = self._slice.byte_length()
        if total <= 0:
            return None
        var grapheme_start = self._grapheme_start_of_last_cluster(total)
        var result = self._slice
        result._data = result._data.unsafe_offset(grapheme_start)
        result._size = total - grapheme_start
        self._slice._size = grapheme_start
        return result

    @doc_hidden
    def _grapheme_start_of_last_cluster(mut self, end: Int) -> Int:
        """Return the byte offset in `self._slice[:end]` where the last
        grapheme cluster begins, using the cached safe-start when available.

        Args:
            end: The end of the range to scan, in `(0, self._slice.byte_length()]`.

        Returns:
            The byte offset `<= end` that begins the last grapheme cluster.
        """
        if not self._back_safe_known or self._back_safe_start >= end:
            self._back_safe_start = self._slice._safe_grapheme_start(end)
            self._back_safe_known = True
        return self._slice._prev_grapheme_start(end, self._back_safe_start)
