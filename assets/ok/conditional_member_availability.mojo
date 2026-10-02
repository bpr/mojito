# A member's `where` clause decides whether an instance has it: `Slot[Int]`
# default-constructs and resets, and `Slot[NoDefault]` has neither member
# and is built from a value.
struct NoDefault(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


struct Slot[T: Movable & Deinitable]:
    var item: Self.T

    def __init__(out self) where conforms_to(Self.T, Defaultable):
        self.item = Self.T()

    def __init__(out self, var item: Self.T):
        self.item = item^

    def reset(mut self) where conforms_to(Self.T, Defaultable):
        self.item = Self.T()


def main():
    var a = Slot[Int](5)
    print(a.item)
    a.reset()
    print(a.item)
    var b = Slot[Int]()
    print(b.item)
    var c = Slot[NoDefault](NoDefault(7))
    print(c.item.v)
