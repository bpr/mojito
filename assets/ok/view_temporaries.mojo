# View temporaries as receivers and arguments, upstream's temporary-lifetime
# rules: a plain `self` method writes through a mutable-origin pointer field;
# a `ref[self]`-returning call on a temporary receiver chains (the temporary
# lives for the statement) and its result may be discarded; a subscript view
# passed straight to a method or free call at its source's last use keeps the
# source alive through the call; a temporary holding a pointer to a caller
# local prints. (`Writer.write` of a view or struct argument is VM-only:
# conformance/fixtures/view_temporary_write.mojo.)
from std.format._utils import FormatStruct, Named

struct Cell[o: Origin[mut=True]](Movable):
    var p: Pointer[Int, Self.o]

    def __init__(out self, ref[Self.o] v: Int):
        self.p = Pointer(to=v)

    def bump(self):
        self.p[] = self.p[] + 1

    def get(self) -> Int:
        return self.p[]


@fieldwise_init
struct Point(Writable, Movable):
    var x: Int
    var y: Bool

    def write_to(self, mut writer: Some[Writer]):
        FormatStruct(writer, "Point").params(2, "tag").fields(self.x, self.y)


@fieldwise_init
struct Stepwise(Writable, Movable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        var format = FormatStruct(writer, "Stepwise")
        format.params(1)
        _ = format.params(2)
        format.fields(self.x)


struct Sink(Movable):
    var total: Int

    def __init__(out self):
        self.total = 0

    def take(mut self, view: StringSpan):
        self.total += view.byte_length()


def byte_count(view: StringSpan) -> Int:
    return view.byte_length()


def main():
    var v = 1
    var c = Cell(v)
    c.bump()
    c.bump()
    print(c.get())
    print(Point(3, True))
    print(Stepwise(4))
    var s = String("hello")
    var sink = Sink()
    sink.take(s[byte=1:3])
    print(sink.total)
    var t = String("world")
    print(byte_count(t[byte=0:2]))
    var w = 7
    print(Named("n", w))
    var u = String("mojito")
    var head = u[byte=0:3]
    var tail = u[byte=3:6]
    print(head)
    print(tail)
