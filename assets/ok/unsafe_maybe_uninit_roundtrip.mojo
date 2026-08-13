# UnsafeMaybeUninit inline-uninit storage: value constructor, unsafe_write,
# the deinit/ref unsafe_assume_init overload pair, unsafe_deinit running the
# payload destructor, and the leak-by-design paths (plain discard,
# unsafe_forget, and unsafe_write over an initialized payload) where the
# destructor must NOT run.
from std.memory import UnsafeMaybeUninit

struct Recorder(Movable, Deinitable):
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("deinit", self.id)

def main():
    # Default ctor + write + consuming take.
    var a = UnsafeMaybeUninit[Int]()
    a.unsafe_write(42)
    print(a^.unsafe_assume_init())

    # Value ctor + borrowing read + consuming take of the same value.
    var b = UnsafeMaybeUninit[Recorder](Recorder(1))
    print("borrowed", b.unsafe_assume_init().id)
    var taken = b^.unsafe_assume_init()
    print("took", taken.id)

    # unsafe_deinit runs the payload destructor ("deinit 2").
    var c = UnsafeMaybeUninit[Recorder](Recorder(2))
    c^.unsafe_deinit()

    # Discarding initialized storage leaks: no "deinit 3".
    var d = UnsafeMaybeUninit[Recorder](Recorder(3))
    _ = d^

    # unsafe_forget: the explicit spelling of the same leak.
    var e = UnsafeMaybeUninit[Recorder](Recorder(4))
    e^.unsafe_forget()

    # Overwriting initialized storage leaks the old payload: no "deinit 5".
    var f = UnsafeMaybeUninit[Recorder]()
    f.unsafe_write(Recorder(5))
    f.unsafe_write(Recorder(6))
    print("overwrote to", f^.unsafe_assume_init().id)
