# Upstream's `std/builtin/builtin_slice.mojo`. `Slice` and the `slice(...)`
# constructor are still compiler-provided descriptor types (prelude names,
# with intrinsic `indices`, equality, and `Slice(start, end, step)` writing);
# `ContiguousSlice` and `StridedSlice` are the library structs upstream
# declares, visible only where an import binds them. A slice literal calls
# its parameter type's `__slice_literal__` constructor: `xs[a:b]` builds a
# `ContiguousSlice` for a `ContiguousSlice` parameter, and `xs[a:b:c]` a
# `StridedSlice` for a `StridedSlice` one.


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
