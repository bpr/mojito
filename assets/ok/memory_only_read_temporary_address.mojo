# A memory-only temporary at a `read` parameter is passed by the address of
# its storage, as at Mojo: a capturing lambda's storage, a call's result,
# and a construction's are each read where they lie and destroyed once,
# when the call returns.
struct Noisy(Copyable, Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 100
        print("copy", self.v)

    def __init__(out self, *, deinit move: Self):
        self.v = move.v + 1000
        print("move", self.v)

    def __deinit__(deinit self):
        print("del", self.v)


def twice[F: def() -> Int](f: F) -> Int:
    return f() * 10 + f()


@no_inline
def make(v: Int) -> Noisy:
    return Noisy(v)


@no_inline
def show(n: Noisy) -> Int:
    print("show", n.v)
    return n.v


def main():
    var s = String("ab")
    print(twice(lambda {var s^} -> Int: (s.__iadd__("!"), s.byte_length())[1]))
    print(show(make(1)))
    print(show(Noisy(2)))
    print("end")
