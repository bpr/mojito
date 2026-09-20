# A per-instantiation method clone inherits its checked template's facts when
# the body calls a reference-returning method on a field of `self` (`docs/
# notes/instantiation-from-template.md`, class MethodBody, feature
# `reference_calls`): a subscript or an accessor passing scalars, either
# forwarded as the method's own reference result or read by value. The
# reference the call yields names `self`, so a template keeps it by owner and
# an instance gets its own receiver back; a by-value read owes the implicit
# copy at the instance's type.
@fieldwise_init
struct Token(Movable):
    var id: Int


# The template marks no copyable read: `T` is only movable. The `Int` instance
# marks one and the `Token` instance does not, as inference would.
struct Rack[T: Movable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def at(
        ref self, index: Int
    ) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        return self.items[index]


struct Shelf[T: ImplicitlyCopyable & Deinitable]:
    var items: List[Self.T]

    def __init__(out self):
        self.items = List[Self.T]()

    def add(mut self, var value: Self.T):
        self.items.append(value^)

    def at(
        ref self, index: Int
    ) -> ref[origin_of(self.items)._get_owned_interior["element"]] Self.T:
        return self.items[index]

    def first(self) -> Self.T:
        return self.items[0]

    def second(self) -> Self.T:
        var held = self.items.unsafe_get(1)
        return held


def main():
    var numbers = Shelf[Int]()
    numbers.add(3)
    numbers.add(4)
    print(numbers.at(1))
    print(numbers.first(), numbers.second())

    var counts = Rack[Int]()
    counts.add(8)
    print(counts.at(0))

    var tokens = Rack[Token]()
    tokens.add(Token(9))
    print(tokens.at(0).id)

    var words = Shelf[String]()
    words.add("x")
    words.add("y")
    print(words.at(1))
    print(words.first(), words.second())
