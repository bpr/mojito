# Nominal range family used by the compiler prelude, mirroring current Mojo's
# three private range structs (std/builtin/range.mojo): a zero-starting form,
# a sequential form whose two-argument spelling never counts down, and a
# strided form whose zero step canonicalizes to the empty range. Each struct
# is its own borrowed iterator over `Scalar[dtype]` elements. A float dtype
# takes the strided form only, through `_FloatStridedRange`. `reversed()`/
# `bounds()` on float ranges and non-Int `Indexer` arguments to `range` are
# recorded subset gaps, and `__getitem__` stays unchecked like the wider
# subset's debug asserts.

from std.iter import Iterable, Iterator, StopIteration

def _range_length(start: Int, stop: Int, step: Int) -> Int:
    if step == 0:
        return 0
    if step > 0:
        if start >= stop:
            return 0
        return (stop - start + step - 1) // step
    if start <= stop:
        return 0
    var stride = -step
    return (start - stop + stride - 1) // stride

struct _ZeroStartingRange[dtype: DType = DType.int](
    Copyable, Deinitable, ImplicitlyCopyable, Iterable, Iterator, Movable
):
    comptime Element = Scalar[Self.dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var curr: Scalar[Self.dtype]
    var end: Scalar[Self.dtype]

    def __init__(out self, end: Scalar[Self.dtype]):
        var clamped = end
        # Scalar comparisons produce width-1 masks; branch through Int.
        if Int(clamped) < 0:
            clamped = 0
        self.curr = clamped
        self.end = clamped

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def __reversed__(self) -> _StridedRange[Self.dtype]:
        return _StridedRange[Self.dtype](self.end - 1, Scalar[Self.dtype](-1), Scalar[Self.dtype](-1))

    def __next__(mut self) raises StopIteration -> Scalar[Self.dtype]:
        var remaining = self.curr
        if Int(remaining) == 0:
            raise StopIteration()
        self.curr = remaining - 1
        return self.end - remaining

    def __len__(self) -> Int:
        return Int(self.curr)

    def __getitem__(self, idx: Int) -> Scalar[Self.dtype]:
        return Scalar[Self.dtype](idx)

struct _SequentialRange[dtype: DType = DType.int](
    Copyable, Deinitable, ImplicitlyCopyable, Iterable, Iterator, Movable
):
    comptime Element = Scalar[Self.dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var start: Scalar[Self.dtype]
    var end: Scalar[Self.dtype]

    def __init__(out self, start: Scalar[Self.dtype], end: Scalar[Self.dtype]):
        self.start = start
        var stop = end
        if Int(stop) < Int(start):
            stop = start
        self.end = stop

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def __reversed__(self) -> _StridedRange[Self.dtype]:
        return _StridedRange[Self.dtype](self.end - 1, self.start - 1, Scalar[Self.dtype](-1))

    def __next__(mut self) raises StopIteration -> Scalar[Self.dtype]:
        var current = self.start
        if Int(current) == Int(self.end):
            raise StopIteration()
        self.start = current + 1
        return current

    def __len__(self) -> Int:
        return Int(self.end) - Int(self.start)

    def __getitem__(self, idx: Int) -> Scalar[Self.dtype]:
        return self.start + Scalar[Self.dtype](idx)

struct _StridedRange[dtype: DType = DType.int](
    Copyable, Deinitable, ImplicitlyCopyable, Iterable, Iterator, Movable
):
    comptime Element = Scalar[Self.dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var start: Scalar[Self.dtype]
    var end: Scalar[Self.dtype]
    var step: Scalar[Self.dtype]

    def __init__(
        out self, start: Scalar[Self.dtype], end: Scalar[Self.dtype], step: Scalar[Self.dtype]
    ):
        # A zero step has no direction; collapse it to the canonical empty
        # range at construction (upstream's rule), keeping the check out of
        # `__next__` and the division out of `__len__`.
        var first = start
        var last = end
        var stride = step
        if Int(stride) == 0:
            first = 0
            last = 0
            stride = 1
        self.start = first
        self.end = last
        self.step = stride

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def __reversed__(self) -> _StridedRange[Self.dtype]:
        var last = self.start + Scalar[Self.dtype](len(self) - 1) * self.step
        return _StridedRange[Self.dtype](last, self.start - self.step, -self.step)

    def __next__(mut self) raises StopIteration -> Scalar[Self.dtype]:
        if Int(self.step) > 0:
            if Int(self.start) >= Int(self.end):
                raise StopIteration()
        else:
            if Int(self.end) >= Int(self.start):
                raise StopIteration()
        var result = self.start
        self.start += self.step
        return result

    def __len__(self) -> Int:
        return _range_length(Int(self.start), Int(self.end), Int(self.step))

    def __getitem__(self, idx: Int) -> Scalar[Self.dtype]:
        return self.start + Scalar[Self.dtype](idx) * self.step

# A floating-point strided range iterates by index, as upstream's float path
# does: element `k` is the fused multiply-add `k * step + start`, so the
# sequence never drifts, and it has `ceil((end - start) / step)` elements
# (none for a zero step).
struct _FloatStridedRange[dtype: DType = DType.float64](
    Copyable, Deinitable, ImplicitlyCopyable, Iterable, Iterator, Movable
):
    comptime Element = Scalar[Self.dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var start: Scalar[Self.dtype]
    var step: Scalar[Self.dtype]
    var idx: Int
    var count: Int

    def __init__(
        out self, start: Scalar[Self.dtype], end: Scalar[Self.dtype], step: Scalar[Self.dtype]
    ):
        self.start = start
        self.step = step
        self.idx = 0
        self.count = 0
        # Scalar comparisons produce width-1 masks; branch through Float64.
        if Float64(step) != 0.0:
            var raw = ((end - start) / step).__ceil__()
            if Float64(raw) > 0.0:
                self.count = Int(raw)

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def __next__(mut self) raises StopIteration -> Scalar[Self.dtype]:
        if self.idx >= self.count:
            raise StopIteration()
        var result = Scalar[Self.dtype](self.idx).__fma__(self.step, self.start)
        self.idx += 1
        return result

def range(end: Int) -> _ZeroStartingRange[DType.int]:
    return _ZeroStartingRange[DType.int](end)

def range(start: Int, end: Int) -> _SequentialRange[DType.int]:
    return _SequentialRange[DType.int](start, end)

def range(start: Int, end: Int, step: Int) -> _StridedRange[DType.int]:
    return _StridedRange[DType.int](start, end, step)
