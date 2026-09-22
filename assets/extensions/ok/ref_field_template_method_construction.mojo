# A per-instantiation method clone inherits its checked template's facts when
# the body constructs a view over the receiver — `ref source = self` handed to
# a fieldwise struct's reference field — whose type carries `origin_of(self)`
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `constructions`). The constructed type names the receiver inside an origin
# argument; the template keeps that slot unbound and the origin by binding, so
# an instance writes its own receiver back into the type. The `return`
# re-resolves the annotation's `origin_of(self)` over the body's own places,
# which the instance repeats once. This is the shape of every bundled
# iterator maker (`List.__iter__`, `List.__reversed__`, `Optional.__iter__`,
# `Dict.take_items`). A direct `ref` struct field is the kept extension
# (`docs/non-goals.md`), so the fixture lives here; its `Pointer` twin would
# construct the view over `Pointer(to=self.items)`, a builtin construction the
# derivation does not admit.
@fieldwise_init
struct View[mut: Bool, //, T: Copyable & Deinitable, origin: Origin[mut=mut]](
    Copyable, Movable
):
    var src: ref[origin] Store[Self.T]
    var index: Int

    def first(self) -> Self.T:
        return self.src.items[self.index].copy()

    def count(self) -> Int:
        return len(self.src.items) - self.index


struct Store[T: Copyable & Deinitable](Movable):
    var items: List[Self.T]

    def __init__(out self, var item: Self.T):
        self.items = List[Self.T]()
        self.items.append(item^)

    def view(ref self) -> View[Self.T, origin_of(self)]:
        ref source = self
        return View[Self.T](source, 0)

    def rest(ref self) -> View[Self.T, origin_of(self)]:
        ref source = self
        return View[Self.T](source, len(self.items) - 1)


def main():
    var a = Store[Int](4)
    var b = Store[String](String("q"))
    a.items.append(5)
    b.items.append(String("r"))
    print(a.view().first(), b.view().first(), a.view().count(), b.view().count())
    print(a.rest().first(), b.rest().first(), a.rest().count(), b.rest().count())
