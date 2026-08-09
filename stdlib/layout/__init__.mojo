# The layout package: CPU layouts and layout-aware tensor views (proof
# subset).  `IntTuple` is FLAT with rank <= 4 — a documented divergence from
# upstream's recursive nested modes; unused trailing dims are zero.  All
# state is pointer-free `Int`s, so a `Layout` value can freeze as a
# compile-time parameter.  `LayoutTensor` is a layout-aware view over a
# caller-managed buffer whose compile-time `layout` folds into each
# specialization.  Deferred follow-ups: origin-parameterized borrowed views
# (an origin-bearing UnsafePointer currently designates a single place, so
# the view holds a plain pointer and does not own or free the buffer), the
# GPU surface (address spaces, device passage, async copies), tile/slice
# views, SIMD load/store, the layout algebra (composition, coalesce,
# blocked products), and `idx2crd`/`transpose`.

def _set_dim(mut t: IntTuple, i: Int, value: Int):
    if i == 0:
        t.d0 = value
    elif i == 1:
        t.d1 = value
    elif i == 2:
        t.d2 = value
    elif i == 3:
        t.d3 = value


struct IntTuple(Copyable, Equatable, Movable, Writable):
    var rank: Int
    var d0: Int
    var d1: Int
    var d2: Int
    var d3: Int

    # The field-mirroring constructor (fieldwise shape, hand-written because
    # the convenience arities below coexist with it).
    def __init__(out self, rank: Int, d0: Int, d1: Int, d2: Int, d3: Int):
        self.rank = rank
        self.d0 = d0
        self.d1 = d1
        self.d2 = d2
        self.d3 = d3

    def __init__(out self, d0: Int):
        self.rank = 1
        self.d0 = d0
        self.d1 = 0
        self.d2 = 0
        self.d3 = 0

    def __init__(out self, d0: Int, d1: Int):
        self.rank = 2
        self.d0 = d0
        self.d1 = d1
        self.d2 = 0
        self.d3 = 0

    def __init__(out self, d0: Int, d1: Int, d2: Int):
        self.rank = 3
        self.d0 = d0
        self.d1 = d1
        self.d2 = d2
        self.d3 = 0

    def __init__(out self, d0: Int, d1: Int, d2: Int, d3: Int):
        self.rank = 4
        self.d0 = d0
        self.d1 = d1
        self.d2 = d2
        self.d3 = d3

    def __len__(self) -> Int:
        return self.rank

    def __getitem__(self, i: Int) raises -> Int:
        if i < 0 or i >= self.rank:
            raise Error("IntTuple index out of range")
        return self._dim(i)

    # Non-raising internal read: 0 outside the rank.  Layout's mapping and
    # size arithmetic stay non-raising through this.
    def _dim(self, i: Int) -> Int:
        if i == 0:
            return self.d0
        if i == 1:
            return self.d1
        if i == 2:
            return self.d2
        if i == 3:
            return self.d3
        return 0

    def __eq__(self, other: Self) -> Bool:
        if self.rank != other.rank:
            return False
        var i = 0
        while i < self.rank:
            if self._dim(i) != other._dim(i):
                return False
            i += 1
        return True

    def __ne__(self, other: Self) -> Bool:
        return not (self == other)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("(")
        var i = 0
        while i < self.rank:
            if i > 0:
                writer.write(", ")
            writer.write(self._dim(i))
            i += 1
        writer.write(")")

@fieldwise_init
struct Layout(Copyable, Equatable, Movable, Writable, def(IntTuple) -> Int):
    var shape: IntTuple
    var stride: IntTuple

    # The factory bodies read fields directly (no method calls) so they stay
    # inside the compile-time purity walk: `Layout.row_major(2, 3)` must
    # evaluate at elaboration when used as a value parameter.
    @staticmethod
    def row_major(*dims: Int) -> Layout:
        var shape = IntTuple(0, 0, 0, 0, 0)
        var count = 0
        for d in dims:
            if count < 4:
                _set_dim(shape, count, d)
            count += 1
        shape.rank = count
        var stride = IntTuple(count, 0, 0, 0, 0)
        if count == 1:
            stride.d0 = 1
        elif count == 2:
            stride.d0 = shape.d1
            stride.d1 = 1
        elif count == 3:
            stride.d0 = shape.d1 * shape.d2
            stride.d1 = shape.d2
            stride.d2 = 1
        elif count == 4:
            stride.d0 = shape.d1 * shape.d2 * shape.d3
            stride.d1 = shape.d2 * shape.d3
            stride.d2 = shape.d3
            stride.d3 = 1
        return Layout(shape, stride)

    @staticmethod
    def col_major(*dims: Int) -> Layout:
        var shape = IntTuple(0, 0, 0, 0, 0)
        var count = 0
        for d in dims:
            if count < 4:
                _set_dim(shape, count, d)
            count += 1
        shape.rank = count
        var stride = IntTuple(count, 0, 0, 0, 0)
        if count >= 1:
            stride.d0 = 1
        if count >= 2:
            stride.d1 = shape.d0
        if count >= 3:
            stride.d2 = shape.d0 * shape.d1
        if count >= 4:
            stride.d3 = shape.d0 * shape.d1 * shape.d2
        return Layout(shape, stride)

    def rank(self) -> Int:
        return self.shape.rank

    def size(self) -> Int:
        var total = 1
        var i = 0
        while i < self.shape.rank:
            total *= self.shape._dim(i)
            i += 1
        return total

    # One past the largest linear index the layout can produce.
    def cosize(self) -> Int:
        var last = 0
        var i = 0
        while i < self.shape.rank:
            if self.shape._dim(i) > 0:
                last += (self.shape._dim(i) - 1) * self.stride._dim(i)
            i += 1
        return last + 1

    # Map logical coordinates to the linear memory index.
    def __call__(self, idx: IntTuple) -> Int:
        var linear = 0
        var i = 0
        while i < self.shape.rank:
            linear += idx._dim(i) * self.stride._dim(i)
            i += 1
        return linear

    def __eq__(self, other: Self) -> Bool:
        return self.shape == other.shape and self.stride == other.stride

    def __ne__(self, other: Self) -> Bool:
        return not (self == other)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("(")
        self.shape.write_to(writer)
        writer.write(":")
        self.stride.write_to(writer)
        writer.write(")")


struct LayoutTensor[dtype: DType, layout: Layout](Copyable, Movable):
    var ptr: UnsafePointer[Scalar[dtype]]
    var runtime_layout: Layout

    def __init__(out self, ptr: UnsafePointer[Scalar[dtype]]):
        self.ptr = ptr
        self.runtime_layout = layout

    def size(self) -> Int:
        return self.runtime_layout.size()

    def dim(self, i: Int) -> Int:
        return self.runtime_layout.shape._dim(i)

    def __getitem__(self, i: Int) -> Scalar[dtype]:
        var map = self.runtime_layout
        return self.ptr[map(IntTuple(i))]

    def __getitem__(self, i: Int, j: Int) -> Scalar[dtype]:
        var map = self.runtime_layout
        return self.ptr[map(IntTuple(i, j))]

    def __setitem__(mut self, i: Int, value: Scalar[dtype]):
        var map = self.runtime_layout
        self.ptr[map(IntTuple(i))] = value

    def __setitem__(mut self, i: Int, j: Int, value: Scalar[dtype]):
        var map = self.runtime_layout
        self.ptr[map(IntTuple(i, j))] = value
