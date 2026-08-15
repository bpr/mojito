# Nominal range family used by the compiler prelude, mirroring current Mojo's
# three private range structs (std/builtin/range.mojo): a zero-starting form,
# a sequential form whose two-argument spelling never counts down, and a
# strided form whose zero step canonicalizes to the empty range. Each struct
# is its own borrowed iterator over `Scalar[dtype]` elements. The proof
# subset is integral-only: float strided ranges, `reversed()`/`bounds()`/
# `__has_next__`, and non-Int `Indexer` arguments to `range` are recorded
# subset gaps, and `__getitem__` stays unchecked like the wider subset's
# debug asserts.

from std.iterable import Iterable, Iterator, StopIteration

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
    comptime Element = Scalar[dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var curr: Scalar[dtype]
    var end: Scalar[dtype]

    def __init__(out self, end: Scalar[dtype]):
        var clamped = end
        # Scalar comparisons produce width-1 masks; branch through Int.
        if Int(clamped) < 0:
            clamped = 0
        self.curr = clamped
        self.end = clamped

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def __next__(mut self) raises StopIteration -> Scalar[dtype]:
        var remaining = self.curr
        if Int(remaining) == 0:
            raise StopIteration()
        self.curr = remaining - 1
        return self.end - remaining

    def __len__(self) -> Int:
        return Int(self.curr)

    def __getitem__(self, idx: Int) -> Scalar[dtype]:
        return Scalar[dtype](idx)

struct _SequentialRange[dtype: DType = DType.int](
    Copyable, Deinitable, ImplicitlyCopyable, Iterable, Iterator, Movable
):
    comptime Element = Scalar[dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var start: Scalar[dtype]
    var end: Scalar[dtype]

    def __init__(out self, start: Scalar[dtype], end: Scalar[dtype]):
        self.start = start
        var stop = end
        if Int(stop) < Int(start):
            stop = start
        self.end = stop

    def __iter__(ref self) -> Self.IteratorType[origin_of(self)]:
        return self

    def __next__(mut self) raises StopIteration -> Scalar[dtype]:
        var current = self.start
        if Int(current) == Int(self.end):
            raise StopIteration()
        self.start = current + 1
        return current

    def __len__(self) -> Int:
        return Int(self.end) - Int(self.start)

    def __getitem__(self, idx: Int) -> Scalar[dtype]:
        return self.start + Scalar[dtype](idx)

struct _StridedRange[dtype: DType = DType.int](
    Copyable, Deinitable, ImplicitlyCopyable, Iterable, Iterator, Movable
):
    comptime Element = Scalar[dtype]
    comptime IteratorType[
        iterable_mut: Bool, //, iterable_origin: Origin[mut=iterable_mut]
    ] = Self
    var start: Scalar[dtype]
    var end: Scalar[dtype]
    var step: Scalar[dtype]

    def __init__(
        out self, start: Scalar[dtype], end: Scalar[dtype], step: Scalar[dtype]
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

    def __next__(mut self) raises StopIteration -> Scalar[dtype]:
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

    def __getitem__(self, idx: Int) -> Scalar[dtype]:
        return self.start + Scalar[dtype](idx) * self.step

def range(end: Int) -> _ZeroStartingRange[DType.int]:
    return _ZeroStartingRange[DType.int](end)

def range(start: Int, end: Int) -> _SequentialRange[DType.int]:
    return _SequentialRange[DType.int](start, end)

def range(start: Int, end: Int, step: Int) -> _StridedRange[DType.int]:
    return _StridedRange[DType.int](start, end, step)
