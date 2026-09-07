# `reversed(value)`: upstream's `std/builtin/reversed.mojo` surface for the
# proof subset — the three range structs (`__reversed__` yields the strided
# range that walks back), `List` (a borrowed back-to-front iterator), and
# `String`/`StringSpan` (the grapheme clusters back to front).
from std.collections.list import _ListReversedIter
from std.range import _SequentialRange, _StridedRange, _ZeroStartingRange
from std.string import String, StringSpan, _GraphemeReversedIter


def reversed(value: _ZeroStartingRange[DType.int]) -> _StridedRange[DType.int]:
    return value.__reversed__()


def reversed(value: _SequentialRange[DType.int]) -> _StridedRange[DType.int]:
    return value.__reversed__()


def reversed(value: _StridedRange[DType.int]) -> _StridedRange[DType.int]:
    return value.__reversed__()


def reversed[T: Copyable & Movable](ref value: List[T]) -> _ListReversedIter[T, origin_of(value)]:
    return value.__reversed__()


def reversed(ref value: String) -> _GraphemeReversedIter[origin_of(value)]:
    return _GraphemeReversedIter(StringSpan(value), value.size, 0, False)


def reversed(ref value: StringSpan) -> _GraphemeReversedIter[origin_of(value)]:
    return _GraphemeReversedIter(value, value.byte_length(), 0, False)
