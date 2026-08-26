# MaybeUninit inline storage lowers payload-only natively (no init
# flag, no synthesized declaration): writes move payloads in raw, the
# deinit/ref unsafe_assume_init pair reads and takes through the payload
# projection, unsafe_deinit runs the destructor in place, and every
# leak-by-design path (plain discard, unsafe_forget, overwrite of an
# initialized payload) runs NO destructor — matching the VM's no-op
# UninitStorage drop. One payload type only: two instantiations of a
# generic template still collide pre-canonicalization (the mono guard).
from std.memory import MaybeUninit

struct Recorder(Movable, Deinitable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("deinit", self.id)

def main():
    # Default ctor + write + consuming take.
    var a = MaybeUninit[Recorder]()
    a.unsafe_write(Recorder(42))
    print("took", a^.unsafe_assume_init().id)

    # Value ctor + borrowing read + consuming take of the same value.
    var b = MaybeUninit[Recorder](Recorder(1))
    print("borrowed", b.unsafe_assume_init().id)
    print("retook", b^.unsafe_assume_init().id)

    # unsafe_deinit runs the payload destructor ("deinit 2").
    var c = MaybeUninit[Recorder](Recorder(2))
    c^.unsafe_deinit()

    # Discard, forget, and overwrite all leak: no "deinit 3"/"deinit 4"/
    # "deinit 5".
    var d = MaybeUninit[Recorder](Recorder(3))
    _ = d^
    var e = MaybeUninit[Recorder](Recorder(4))
    e^.unsafe_forget()
    var f = MaybeUninit[Recorder]()
    f.unsafe_write(Recorder(5))
    f.unsafe_write(Recorder(6))
    print("overwrote to", f^.unsafe_assume_init().id)
