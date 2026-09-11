# `reversed(value)`: upstream's `std/builtin/reversed.mojo` surface for the
# proof subset — the three range structs (`__reversed__` yields the strided
# range that walks back) and `List` (a borrowed back-to-front iterator).
# `String` and `StringSpan` define `__reversed__` but, as upstream, do not
# conform to `ReversibleRange`, so `reversed(s)` has no overload for them.
from std.collections.list import _ListReversedIter
from std.range import _SequentialRange, _StridedRange, _ZeroStartingRange


def reversed(value: _ZeroStartingRange[DType.int]) -> _StridedRange[DType.int]:
    return value.__reversed__()


def reversed(value: _SequentialRange[DType.int]) -> _StridedRange[DType.int]:
    return value.__reversed__()


def reversed(value: _StridedRange[DType.int]) -> _StridedRange[DType.int]:
    return value.__reversed__()


def reversed[T: Copyable & Movable](ref value: List[T]) -> _ListReversedIter[T, origin_of(value)]:
    return value.__reversed__()
