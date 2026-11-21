# `Pointer.unsafe_write(init_with=)` builds the pointee in place, so a
# non-`Movable` element can be written, and `unsafe_deinit_pointee_with`
# hands the pointee to a consuming function: both are upstream's declared
# bodies.
from std.memory.alloc import unsafe_alloc


@fieldwise_init
struct Pinned(not Movable):
    var value: Int


struct Token(Movable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __init__(out self, *, deinit move: Self):
        self.id = move.id
        print("move", self.id)

    def finish(deinit self):
        print("finish", self.id)


def finish(var token: Token):
    token^.finish()


def main():
    var ptr = unsafe_alloc[Pinned](1)
    ptr.unsafe_write(init_with=lambda () -> Pinned: Pinned(7))
    print(ptr[].value)
    ptr.unsafe_deinit_pointee()
    ptr.unsafe_free()
    var tp = unsafe_alloc[Token](1)
    tp.unsafe_write(init_with=lambda () -> Token: Token(3))
    tp.unsafe_deinit_pointee_with(finish)
    tp.unsafe_free()
    print("end")
