# Upstream's `std/builtin/builtin_slice.mojo`. `Slice` and the `slice(...)`
# functions are prelude names; `ContiguousSlice` and `StridedSlice` are
# visible only where an import binds them. A slice literal calls its
# parameter type's `__slice_literal__` constructor: `xs[a:b]` builds a
# `ContiguousSlice` for a `ContiguousSlice` parameter, `xs[a:b:c]` a
# `StridedSlice` for a `StridedSlice` one, and any literal a `Slice` for a
# `Slice` one.

from std.format._utils import FormatStruct, Named


# A slice expression: the value slice syntax builds within square brackets.
struct Slice(
    Equatable,
    ImplicitlyCopyable,
    Writable,
):
    var start: Optional[Int]
    var end: Optional[Int]
    var step: Optional[Int]

    @inline(.always)
    def __init__(out self, start: Int, end: Int):
        self.start = start
        self.end = end
        self.step = None

    @inline(.always)
    def __init__(
        out self,
        start: Optional[Int],
        end: Optional[Int],
        step: Optional[Int],
        __slice_literal__: NoneType = None,
    ):
        self.start = start
        self.end = end
        self.step = step

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        FormatStruct(writer, "Slice").fields(self.start, self.end, self.step)

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        FormatStruct(writer, "Slice").fields(
            Named("start", self.start),
            Named("end", self.end),
            Named("step", self.step),
        )

    @inline(.always)
    def __eq__(self, other: Self) -> Bool:
        return (
            self.start == other.start
            and self.end == other.end
            and self.step == other.step
        )

    # The start, end, and step of the slice applied to a container of
    # `length` elements: negative bounds wrap, `None` bounds default to the
    # container's start or end by the step's sign, and out-of-bounds ones
    # clamp.
    def indices(self, length: Int) -> Tuple[Int, Int, Int]:
        var step = self.step.or_else(1)

        var start = self.start
        var end = self.end

        var positive_step = step > 0

        if not start:
            start = 0 if positive_step else length - 1
        elif start.value() < 0:
            start = start.value() + length
            if start.value() < 0:
                start = 0 if positive_step else -1
        elif start.value() >= length:
            start = length if positive_step else length - 1

        if not end:
            end = length if positive_step else -1
        elif end.value() < 0:
            end = end.value() + length
            if end.value() < 0:
                end = 0 if positive_step else -1
        elif end.value() >= length:
            end = length if positive_step else length - 1

        return (start.value(), end.value(), step)


# A slice expression that has a stride: strided and unstrided slicing select
# different overloads by it.
struct StridedSlice(ImplicitlyCopyable, Writable):
    var _inner: Slice

    # Implicitly convert from a general slice.
    @implicit
    def __init__(out self, other: Slice):
        self._inner = other

    def __init__(
        out self,
        start: Optional[Int],
        end: Optional[Int],
        stride: Int,
        __slice_literal__: NoneType = None,
    ):
        self._inner = Slice(start, end, stride)

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        self._inner.write_to(writer)

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        self._inner.write_repr_to(writer)

    # The start, end, and step of the slice applied to a container of
    # `length` elements.
    def indices(self, length: Int) -> Tuple[Int, Int, Int]:
        return self._inner.indices(length)


# A slice expression without a stride.
struct ContiguousSlice(ImplicitlyCopyable, Writable):
    var start: Optional[Int]
    var end: Optional[Int]

    # `stride` is always `None`: it disambiguates from slices with a stride.
    @inline(.always)
    def __init__(
        out self,
        start: Optional[Int],
        end: Optional[Int],
        stride: NoneType,
        __slice_literal__: NoneType = None,
    ):
        self.start = start
        self.end = end

    @inline(.never)
    def write_to(self, mut writer: Some[Writer]):
        Slice(self.start, self.end, None).write_to(writer)

    @inline(.never)
    def write_repr_to(self, mut writer: Some[Writer]):
        Slice(self.start, self.end, None).write_repr_to(writer)

    # The start and end of the slice applied to a container of `length`
    # elements.
    def indices(self, length: Int) -> Tuple[Int, Int]:
        var start = self.start.or_else(0)
        var end = self.end.or_else(length)
        if start < 0:
            start = max(start + length, 0)
        elif start >= length:
            start = length

        if end < 0:
            end = max(end + length, 0)
        elif end >= length:
            end = length

        return start, end


@inline(.always)
def slice(end: Int) -> Slice:
    return Slice(None, end, None)


@inline(.always)
def slice(start: Int, end: Int) -> Slice:
    return Slice(start, end)


@inline(.always)
def slice(
    start: Optional[Int], end: Optional[Int], step: Optional[Int]
) -> Slice:
    return Slice(start, end, step)
