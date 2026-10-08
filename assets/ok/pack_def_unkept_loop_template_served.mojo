# Every `def` keyed on a type pack is served by its template, whatever its
# body holds. A `comptime for` display element that reads a field off a
# method's result, and a loop-body `comptime` binding that applies a `def` to
# the index, are evaluated per instance below MIR, beside a pack or not.
@fieldwise_init
struct P(Copyable, Movable):
    var v: Int

    def twin(self) -> P:
        return P(self.v * 2)


def twice(n: Int) -> Int:
    return n * 2


def fields[*Ts: Writable](*args: *Ts):
    comptime for p in [P(1).twin().v, 5]:
        print(p, len(args))


def widths[*Ts: Writable](*args: *Ts):
    comptime for i in range(args.__len__()):
        comptime w = twice(i + 1)
        var v = SIMD[DType.int32, w](1)
        print(v, args[i])


def fields_of[n: Int]():
    comptime for p in [P(n).twin().v, n]:
        print(p)


def squares[n: Int]():
    comptime for i in range(n):
        comptime sq = twice(i)
        print(i, sq)


def main():
    fields(1, "a")
    widths(7, "b")
    fields_of[3]()
    squares[2]()
