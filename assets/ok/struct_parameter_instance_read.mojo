# A struct parameter or `comptime` member read through an instance is the
# instance's type argument: through a local, a `self`, an argument whose
# type is still generic, and a field path, as at the pin.
struct W[n: Int](Copyable, Movable):
    comptime twice = Self.n * 2

    def __init__(out self):
        pass


struct T[s: String, f: Float64](Copyable):
    def __init__(out self):
        pass

    def get(self) -> Float64:
        return self.f


struct H(Movable):
    var w: W[2]

    def __init__(out self):
        self.w = W[2]()

    def show(self):
        print(self.w.n, self.w.twice)


def width_of(w: W) -> Int:
    return w.n


def main():
    var w = W[3]()
    print(w.n, w.twice)
    print(width_of(W[5]()))
    var t = T["a", 2.5]()
    print(t.get(), t.s)
    var h = H()
    h.show()
    print(h.w.n + h.w.twice)
