# A generic `def` called at a loan-carrying argument (`keep(m, p)` with
# `p: Pointer[List[Int], ImmOrigin(origin_of(xs))]`) is served by its
# template, as a method of a loan-carrying instance is: the call mints no
# clone. A store of a copy names no argument expression, so the `def`'s
# transfer summary carries the stored type, and the call closes it with the
# types it binds to the `def`'s own parameters: the places that type names
# are what the destination borrows. `xs` stays alive while a list holding a
# pointer to it is still read. `feed` reaches the store through a method
# with a parameter of its own, and `wrap` builds an instance only its body
# names.
struct Sink(Movable):
    var count: Int

    def __init__(out self):
        self.count = 0

    def put[U: Copyable & Deinitable](mut self, mut into: List[U], value: U):
        into.append(value.copy())
        self.count += 1


struct Box[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def get(self) -> Self.T:
        return self.item.copy()


def keep[T: Copyable & Deinitable](mut into: List[T], value: T):
    into.append(value.copy())


def dup[T: Copyable & Deinitable](value: T) -> T:
    return value.copy()


def feed[T: Copyable & Deinitable](mut sink: Sink, mut into: List[T], value: T):
    sink.put(into, value)


def wrap[T: Copyable & Deinitable](value: T) -> T:
    var box = Box[T](value.copy())
    return box.get()


def kept():
    var xs: List[Int] = [1, 2, 3]
    var m = List[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    keep(m, p)
    print(len(m), m[0][][1])


def duplicated():
    var xs: List[Int] = [4, 5, 6]
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    var q = dup(p)
    print(q[][2])


def fed():
    var xs: List[Int] = [7, 8, 9]
    var sink = Sink()
    var m = List[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    feed(sink, m, p)
    print(sink.count, len(m), m[0][][0])


def wrapped():
    var xs: List[Int] = [10, 11, 12]
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    var q = wrap(p)
    print(q[][1])


def main():
    kept()
    duplicated()
    fed()
    wrapped()
