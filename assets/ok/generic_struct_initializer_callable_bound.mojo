# A function at an applied generic struct's initializer `def(...)` bound
# binds as its inflated type, as at the pin: a module function, a generic
# one, and a capture-free generic lambda declared in the struct's own
# initializer, through an `@inline` initializer and a free function.


def apply_at[F: def[idx: Int]() -> Int](f: F) -> Int:
    return f[2]()


struct Cell[T: Copyable & Defaultable & Writable & Deinitable](Copyable):
    var value: Self.T

    def __init__[F: def() -> Self.T](out self, *, init_with: F):
        self.value = init_with()

    @inline(.nodebug)
    def __init__[F: def[idx: Int]() -> Self.T](out self, *, at: F):
        self.value = Self.T()
        comptime for i in range(3):
            self.value = at[i]()

    @inline(.nodebug)
    def __init__(out self):
        self = Self(at=lambda[i: Int]() -> Self.T: Self.T())


struct Counter(Copyable):
    var total: Int

    def __init__(out self):
        self.total = apply_at(lambda[i: Int]() -> Int: i * 10)


struct Row[*Ts: Movable & Defaultable & Deinitable](Movable):
    var width: Int

    def __init__[F: def[idx: Int]() -> Self.Ts[idx]](out self, *, init_with: F):
        self.width = 0
        comptime for i in range(Self.width_of()):
            var element = init_with[i]()
            _ = element^
            self.width += 1

    @inline(.nodebug)
    def __init__(out self):
        self = Self(init_with=lambda[i: Int]() -> Self.Ts[i]: Self.Ts[i]())

    @staticmethod
    def width_of() -> Int:
        return 2


def make() -> Int:
    return 41


def make_at[idx: Int]() -> Int:
    return idx + 7


def main():
    print(Cell[Int](init_with=make).value)
    print(Cell[Int](at=make_at).value)
    print(Cell[Int]().value)
    print(Counter().total)
    print(Row[Int, Bool]().width)
    var t = Tuple[Int, Bool]()
    print(t[0], t[1])
