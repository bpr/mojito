# Current Mojo's owning Optional surface: AnyType elements, `init_with=`
# placement construction (no Movable requirement), conditional lifecycle
# conformances, borrowed + owned iteration, linear-capable `map`/`and_then`,
# handler-consumed `deinit_with`, and the `deinit_assert_empty` named
# destructor for the empty case.
from std.optional import Optional

struct Res(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __init__(out self, *, deinit move: Self):
        self.id = move.id

    def __deinit__(deinit self):
        print("drop", self.id)

@explicit_destroy("close Conn")
struct Conn(Movable, Deinitable where False):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def close(deinit self):
        print("close", self.id)

def main():
    var base = 10
    var a = Optional[Int](init_with=lambda () -> Int: base + 5)
    print("a", a.value())

    def make() -> Int:
        return 3
    var b = Optional[Int](init_with=make)
    print("b", b.or_else(-1))

    var c = a.copy()
    print("c", c.take())
    c^.deinit_assert_empty()

    for x in a:
        print("iter", x)

    var d = Optional[Res](Res(7))
    for var item in d^:
        print("owned", item.id)

    var e = Optional[Int](4)
    var f = e^.map[Int](lambda (deinit v: Int) -> Int: v * 10)
    print("mapped", f.value())

    def wrap(deinit v: Int) -> Optional[Int]:
        return Optional[Int](v + 1)
    var g = Optional[Int](6)
    var h = g^.and_then[Int](wrap)
    print("chained", h.value())

    var i = Optional[Res](Res(99))
    i^.deinit_with(lambda (deinit element: Res): element^.__deinit__())

    var k = Optional[Conn](init_with=lambda () -> Conn: Conn(5))
    k^.deinit_with(lambda (deinit element: Conn): element^.close())
    print("done")
