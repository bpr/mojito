# Current Mojo's owning Variant surface: `unwrap`/`unsafe_unwrap` (the
# renamed consuming extraction), `set(init_with=...)` in-place placement
# replacement (the alternative is inferred from the factory result — an
# explicit type parameter rejects upstream), and `deinit_with(handler)`
# consuming teardown through a monomorphic `var`-convention handler for one
# alternative (a runtime tag mismatch aborts).
from std.utils import Variant

struct Res(Movable):
    var id: Int
    def __init__(out self, id: Int):
        self.id = id
    def __init__(out self, *, deinit move: Self):
        self.id = move.id
    def __deinit__(deinit self):
        print("drop", self.id)

def main():
    # unwrap / unsafe_unwrap (renamed from take/unsafe_take)
    var v: Variant[Int, String] = Variant[Int, String](5)
    print("isa Int", v.isa[Int]())
    var got = v.unwrap[Int]()
    print("unwrapped", got)

    # set(init_with=...) in-place placement replacement
    var w: Variant[Int, String] = Variant[Int, String](1)
    var base = 40
    w.set(init_with=lambda () -> Int: base + 2)
    print("set", w.unsafe_unwrap[Int]())

    # deinit_with: a monomorphic handler for the active alternative (spelled
    # as a nested def — a String-typed lambda parameter is a recorded lambda
    # gap)
    def consume_string(var element: String):
        print("consumed")
    var x: Variant[Int, String] = Variant[Int, String](String("hello"))
    x.deinit_with(consume_string)

    # deinit_with with a Res payload through a monomorphic handler
    var y: Variant[Res] = Variant[Res](Res(9))
    y^.deinit_with(lambda (var element: Res): element^.__deinit__())
    print("done")
