# A per-instantiation method clone inherits its checked template's facts when
# the method takes `ref self` and returns a reference (`docs/notes/
# instantiation-from-template.md`, class MethodBody, feature
# `reference_result`): every `return` hands out a field of `self`, or a slot of
# a pointer field, as a handle. The declaration and the statement's syntax
# decide that, whatever the place's type, and the handle is neither copied nor
# moved, so an instance owes nothing at it. The bundled `List` and `Optional`
# accessors are this shape, behind a bounds check that aborts.
@fieldwise_init
struct Token(Movable):
    var id: Int


@fieldwise_init
struct Slot[T: Movable & Deinitable](Movable):
    var item: Self.T
    var uses: Int

    def peek(ref self) -> ref[origin_of(self.item)] Self.T:
        return self.item

    def counter(ref self) -> ref[origin_of(self.uses)] Int:
        return self.uses

    def guarded(ref self, limit: Int) -> ref[origin_of(self.item)] Self.T:
        if self.uses > limit:
            self.uses_exceeded()
        return self.item

    def uses_exceeded(self):
        pass


def main():
    var numbers = Slot(1, 0)
    print(numbers.peek(), numbers.counter(), numbers.guarded(5))

    var words = Slot(String("a"), 0)
    print(words.peek(), words.counter(), words.guarded(5))

    var tokens = Slot(Token(3), 0)
    print(tokens.peek().id, tokens.counter(), tokens.guarded(5).id)

    var items = List[String]()
    items.append("x")
    items.append("y")
    items[0] = "z"
    print(items[0], items.unsafe_get(1))

    var maybe = Optional[Int](4)
    print(maybe.value(), maybe.unsafe_value())
