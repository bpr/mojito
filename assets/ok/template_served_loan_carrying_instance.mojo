# An instance over a loan-carrying argument (`Bag[Pointer[List[Int],
# ImmOrigin(origin_of(xs))]]`) is served by its template's methods, as a
# plain-data instance is: no method is cloned for it. A template's store of a
# value of its parameter type names no loan, so its transfer summary carries
# the stored type instead, and the call closes that type with the receiver's
# arguments: the places it then names are what the receiver borrows. `xs` is
# last named before the calls below, and stays alive while a bag or a cell
# holding a pointer to it is still read. `push_copy` stores a copy, which no
# argument expression hands over, and `fill` reaches the store through a
# generic `def`.
struct Bag[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def push(mut self, var value: Self.T):
        self.items.append(value^)

    def push_copy(mut self, value: Self.T):
        self.push(value.copy())

    def first(self) -> Self.T:
        return self.items[0].copy()


struct Cell[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var first: Self.T):
        self.item = first^

    def put(mut self, var value: Self.T):
        self.item = value^


def fill[T: Copyable & Deinitable](mut b: Bag[T], var value: T):
    b.push_copy(value)
    b.push(value^)


def pushed():
    var xs: List[Int] = [1, 2, 3]
    var b = Bag[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    b.push(p)
    print(len(b.items), b.items[0][][1])


def copied():
    var xs: List[Int] = [4, 5, 6]
    var b = Bag[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    b.push_copy(p)
    print(len(b.items), b.items[0][][1])


def filled():
    var xs: List[Int] = [7, 8, 9]
    var b = Bag[Pointer[List[Int], ImmOrigin(origin_of(xs))]]()
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    fill(b, p)
    var f = b.first()
    print(len(b.items), f[][2])


def optional():
    var xs: List[Int] = [10, 11, 12]
    var c = Cell[Optional[Pointer[List[Int], ImmOrigin(origin_of(xs))]]](None)
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    print(1 if c.item else 0)
    c.put(p)
    print(1 if c.item else 0, len(c.item.value()[]))


def main():
    pushed()
    copied()
    filled()
    optional()
