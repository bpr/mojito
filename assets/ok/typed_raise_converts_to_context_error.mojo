# A typed raise converts to the error its context propagates, through that
# type's `@implicit` constructor, as upstream's `emitRaise` converts a
# raised value to its error slot's type: a call raising `StopIteration` or
# a `Writable` struct in a plain `raises` function raises the `Error`
# `Error.__init__[*Ts: Writable]` builds from it, and so does a `raise` of
# one. The pin prints the same lines.
def next_of(x: Int) raises StopIteration -> Int:
    if x > 0:
        raise StopIteration()
    return 7


def call_typed(x: Int) raises:
    print(next_of(x))


def raise_typed() raises:
    raise StopIteration()


@fieldwise_init
struct Code(Movable, Writable):
    var code: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Code(", self.code, ")")


def raise_code() raises Code:
    raise Code(3)


def call_code() raises:
    raise_code()


struct Cells:
    var n: Int

    def __init__(out self):
        self.n = 1

    def __getitem__(self, i: Int) raises StopIteration -> Int:
        if i > 0:
            raise StopIteration()
        return self.n

    def __setitem__(mut self, i: Int, v: Int):
        self.n = v


def subscript() raises:
    var cells = Cells()
    cells[0] += 4
    print(cells[0])
    print(cells[1])


def second[*values: Int]() raises -> Int:
    var it = values.__iter__()
    _ = it.__next__()
    return it.__next__()


def raise_int() raises:
    raise 3


def main():
    try:
        call_typed(0)
        call_typed(1)
    except e:
        print("caught", e)
    try:
        raise_typed()
    except e:
        print("caught", e)
    try:
        call_code()
    except e:
        print("caught", e)
    try:
        subscript()
    except e:
        print("caught", e)
    try:
        print(second[3, 4]())
        print(second[3]())
    except e:
        print("caught", e)
    try:
        raise_int()
    except e:
        print("caught", e)
    var converted: Error = StopIteration()
    print(converted)
    var counted: Error = 5
    print(counted)
