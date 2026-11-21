# expect: invalid call to 'unsafe_deinit_pointee': violated constraint
# `Pointer.unsafe_deinit_pointee` is declared `where ... and
# conforms_to(Self.T, Deinitable)`, which an `@explicit_destroy` element
# violates, as at the pin.
from std.memory.alloc import unsafe_alloc


@explicit_destroy("finish it")
struct Linear[T: AnyType](Movable, Deinitable where False):
    var token: Int

    def __init__(out self):
        self.token = 1

    def finish(deinit self):
        pass


def main():
    var p = unsafe_alloc[Linear[Int]](1)
    p.unsafe_write(Linear[Int]())
    p.unsafe_deinit_pointee()
    p.unsafe_free()
