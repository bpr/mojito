# A pack-keyed `def` spreading its pack into a constructor is served by its
# template, as at the pin: the construction selects the struct's declared
# `__init__` and binds the forwarded pack whole to its type-pack collector.
struct Bag[*Ts: Movable & Deinitable](Movable):
    var n: Int

    def __init__(out self, var *a: *Self.Ts):
        self.n = len(a)


struct Box(Movable):
    var n: Int

    def __init__(out self):
        self.n = -1

    def __init__[*Us: Movable & Deinitable](out self, var *a: *Us):
        self.n = len(a)


struct Sink:
    var n: Int

    def __init__[*Ts: Writable](out self, *a: *Ts):
        self.n = len(a)


def repack[*Ts: Movable](var *args: *Ts) -> Tuple[*Ts]:
    return Tuple[*Ts](*args^)


def count[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
    var t = Tuple(*args^)
    return len(t)


def bag_explicit[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
    return Bag[*Ts](*args^).n


def bag_inferred[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
    return Bag(*args^).n


def boxed[*Ts: Movable & Deinitable](var *args: *Ts) -> Int:
    return Box(*args^).n


def sunk[*Ts: Writable](*a: *Ts) -> Int:
    return Sink(*a).n


def main():
    var t = repack(3, "seven", True)
    print(t[0], t[1], t[2])
    print(len(repack()))
    print(count(1, "a"), count(1.5), count())
    print(bag_explicit(1, "two", True), bag_inferred(1, "two"))
    print(boxed(1, "two"), boxed())
    print(sunk(1, "x"))
