# An `@implicit` conversion inside a generic struct's method is bound on the
# declaration, where the source is `Self.T`: every instance runs the `Self.T`
# constructor, `Box[Int]` included, though `Wrapper[Int]` also declares one
# over `Int`. The template's MIR serves each instance, so the selection is
# made once.
struct Wrapper[T: ImplicitlyCopyable & Deinitable]:
    var tag: Int

    @implicit
    def __init__(out self, value: Int):
        self.tag = 1

    @implicit
    def __init__(out self, value: Self.T):
        self.tag = 2


@fieldwise_init
struct Box[T: ImplicitlyCopyable & Deinitable]:
    var item: Self.T

    def wrap(self) -> Int:
        var w: Wrapper[Self.T] = self.item
        return w.tag


def main():
    var b = Box[Int](3)
    print(b.wrap())
    var s = Box[String]("x")
    print(s.wrap())
