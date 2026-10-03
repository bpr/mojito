# A generic `def` that builds an instance only its body names and calls its
# compile-time-keyed method: `use` is served by its template, the driver
# reads `Box[T]` off the template's checked types at the loan-carrying
# argument, and the elaborator hands `b.kind()` to that instance's clone.
struct Box[T: Copyable & Deinitable](Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def kind(self) -> Int:
        comptime if conforms_to(Self.T, Hashable):
            return 1
        else:
            return 2


def use[T: Copyable & Deinitable](value: T) -> Int:
    var b = Box[T](value.copy())
    return b.kind()


def main():
    var xs: List[Int] = [7, 8, 9]
    var p: Pointer[List[Int], ImmOrigin(origin_of(xs))] = Pointer(to=xs)
    print(use(p))
