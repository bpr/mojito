@fieldwise_init
struct Cell(Copyable, Movable):
    var value: Int

    def bump(mut self, amount: Int):
        self.value += amount


@fieldwise_init
struct Row(Copyable, Movable):
    var cell: Cell

    def __getitem__(
        ref self, index: Int
    ) raises -> ref[origin_of(self.cell)] Cell:
        if index != 0:
            raise Error("bad column")
        return self.cell


@fieldwise_init
struct Matrix(Copyable, Movable):
    var first: Row
    var last: Int

    def __getitem__(
        ref self, row: Int
    ) raises -> ref[origin_of(self.first)] Row:
        if row != 0:
            raise Error("bad row")
        return self.first

    def __getitem__(ref self, row: Int, columns: Slice) raises -> Int:
        if row < 0:
            raise Error("bad slice row")
        var bounds = columns.indices(10)
        return row * 100 + bounds[0] + bounds[1] + bounds[2]

    def __getitem__(
        ref self, columns: Slice
    ) raises -> ref[origin_of(self.first.cell)] Cell:
        var bounds = columns.indices(1)
        if bounds[0] != 0:
            raise Error("bad reference slice")
        return self.first.cell

    def __getitem__(
        ref self, row: Int, column: Int
    ) raises -> ref[origin_of(self.first.cell)] Cell:
        if row != 0 or column != 0:
            raise Error("bad element")
        return self.first.cell

    def __setitem__(mut self, row: Int, columns: Slice, value: Int) raises:
        if row < 0:
            raise Error("bad set row")
        var bounds = columns.indices(10)
        self.last = row * 100 + bounds[0] + bounds[1] + bounds[2] + value


@fieldwise_init
struct Counter(Copyable, Movable):
    var hits: Int

    def __getitem__(mut self, index: Int) raises -> Int:
        if index < 0:
            raise Error("bad counter index")
        self.hits += 1
        return index


@fieldwise_init
struct PlaceIndex(Copyable, Movable):
    var calls: Int

    def __getitem__(mut self, mut index: Int) -> Int:
        self.calls += 1
        index += 1
        return index


@fieldwise_init
struct CallbackIndex(Copyable, Movable):
    def __getitem__[F: def() -> Int](self, callback: F) -> Int:
        return callback()


@fieldwise_init
struct GenericSink(Copyable, Movable):
    var value: Int

    def __getitem__(self, index: Int) -> Bool:
        return False

    def __setitem__[T: Copyable & Deinitable](
        mut self, index: Int, value: T
    ):
        self.value = index


def main():
    var matrix = Matrix(Row(Cell(40)), 0)

    try:
        matrix[0][0].bump(2)
    except error:
        print("unexpected chain")
    print(matrix.first.cell.value)

    try:
        matrix[:].bump(1)
        matrix[0, 0].bump(1)
    except error:
        print("unexpected reference subscript")
    print(matrix.first.cell.value)

    try:
        print(matrix[3, 1:8:2])
        matrix[3, 1:8:2] = 9
    except error:
        print("unexpected slice")
    print(matrix.last)

    try:
        print(matrix[-1, :])
    except error:
        print("caught getter")

    try:
        matrix[-1, :] = 1
    except error:
        print("caught setter")

    var counter = Counter(0)
    try:
        print(counter[7])
    except error:
        print("unexpected mut getter")
    print(counter.hits)

    var position = 40
    var place_index = PlaceIndex(0)
    print(place_index[position], place_index.calls, position)

    var captured = 40

    def next_value() {mut captured} -> Int:
        captured += 1
        return captured

    print(CallbackIndex()[next_value], captured)

    var sink = GenericSink(0)
    sink[42] = True
    print(sink.value)
