# An `Atomic[Int]` written into a raw byte buffer and updated through a
# view of it (`unsafe_bitcast[Atomic[Int]]`), as a reference count kept
# beside the bytes it counts is: `fetch_add` and `load` read and update the
# value in place.
from std.atomic import Atomic
from std.memory.alloc import unsafe_alloc


def main():
    var buf = unsafe_alloc[Byte](16)
    var view = buf.unsafe_bitcast[Atomic[Int]]()
    view.unsafe_write(Atomic[Int](1))
    _ = view[].fetch_add(1)
    print(view[].load())
    buf.unsafe_free()
