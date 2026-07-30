# Concrete substitution of a parameterized associated type. `HasWrap` requires a
# type-parameterized `Wrap[T]`; `Holder` instantiates it as `List[T]`. Through a
# generic bound, `C.Wrap[Thing]` resolves to the concrete `List[Thing]` by
# substituting the application's argument into the member's template.
@fieldwise_init
struct Thing(Copyable, Movable):
    var a: Int

trait HasWrap:
    comptime Wrap[T: Copyable & Movable]: AnyType

    def make(self) -> Self.Wrap[Thing]:
        ...

@fieldwise_init
struct Holder(HasWrap):
    comptime Wrap[T: Copyable & Movable] = List[T]
    var v: Int

    def make(self) -> List[Thing]:
        return [Thing(self.v)]

def first_len[C: HasWrap](c: C) -> C.Wrap[Thing]:
    return c.make()

def main():
    var h = Holder(7)
    var xs = first_len(h)
    print(len(xs))
