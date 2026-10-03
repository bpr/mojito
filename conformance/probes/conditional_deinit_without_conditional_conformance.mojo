# Pin gap probe (Mojo 1.6.0.dev2026092105): a `__deinit__` gated on
# `IsTriviallyDeinitable[Self.T]` in a struct that does not make its
# `Deinitable` conformance conditional too. The pin rejects the struct
# ("'Holder[T]' does not implement all requirements for 'Deinitable' ...
# lacking evidence to prove correctness"); Mojito accepts it, prints
# `trivially copyable`, `trivial deinit` (for `Holder[Int]` only), and `x`,
# and destroys `Holder[String]` without its `__deinit__`. Roadmap section 3
# carries the entry.
from std.traits import IsTriviallyCopyable, IsTriviallyDeinitable


struct Holder[T: Copyable & Deinitable](Copyable, Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def describe(self) where IsTriviallyCopyable[Self.T]:
        print("trivially copyable")

    def __deinit__(deinit self) where IsTriviallyDeinitable[Self.T]:
        print("trivial deinit")


def main():
    var a = Holder[Int](3)
    a.describe()
    var b = Holder[String]("x")
    print(b.item)
