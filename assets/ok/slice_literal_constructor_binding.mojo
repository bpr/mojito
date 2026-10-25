# A slice literal calls its parameter type's `__slice_literal__`
# constructor, `T(lower, upper, stride, __slice_literal__=None)`, with
# `None` for an omitted operand: a user struct declaring one binds a
# literal, each written operand converts as a call argument does (a `None`
# stride selects `ContiguousSlice`, an `Optional[Int]` bound passes), an
# inferred parameter takes the literal's default type (`ContiguousSlice`
# without a stride, `Slice` with one), and a slice-literal assignment
# builds its `__setitem__`'s parameter the same way.
from std.builtin.builtin_slice import ContiguousSlice, StridedSlice

struct Window(ImplicitlyCopyable):
    var low: Optional[Int]
    var high: Optional[Int]

    def __init__(
        out self,
        start: Optional[Int],
        end: Optional[Int],
        stride: NoneType,
        __slice_literal__: NoneType = None,
    ):
        self.low = start
        self.high = end

struct Board:
    var cells: Int

    def __init__(out self):
        self.cells = 0

    def __getitem__(self, s: Window) -> Int:
        return s.low.or_else(-1) * 10 + s.high.or_else(-1)

    def __getitem__(self, i: Int, s: StridedSlice) -> Int:
        return i * 100 + s.indices(10)[2]

    def __setitem__(mut self, s: ContiguousSlice, value: Int):
        self.cells = value * 10 + s.end.or_else(-1)

    def __setitem__(mut self, i: Int, s: StridedSlice, value: Int):
        self.cells = i * 1000 + s.indices(10)[0] * 10 + value

struct Echo:
    def __init__(out self):
        pass

    def __getitem__[T: Writable](self, s: T) -> String:
        return String(s)

def main():
    var board = Board()
    print(board[1:3], board[:2], board[4:])
    board[1:3] = 5
    print(board.cells)
    board[7, 1:9:3] = 4
    print(board.cells, board[2, ::2])
    var xs: List[Int] = [0, 1, 2, 3, 4]
    var o: Optional[Int] = 2
    print(len(xs[1:3:None]), len(xs[o:]))
    print(Echo()[1:3], Echo()[1:5:2])
