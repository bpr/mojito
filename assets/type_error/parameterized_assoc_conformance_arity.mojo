# expect: does not match the signature
# A struct's parameterized associated type must match the requirement's
# parameter count. `HasWrap` requires a one-parameter `Wrap[T]`; `Holder`
# defining a two-parameter `Wrap[T, U]` makes the required `make` return type
# `Self.Wrap[Thing]` unresolvable against it, so conformance fails.
@fieldwise_init
struct Thing(Copyable, Movable):
    var a: Int

trait HasWrap:
    comptime Wrap[T: Copyable & Movable]: AnyType

    def make(self) -> Self.Wrap[Thing]:
        ...

@fieldwise_init
struct Holder(HasWrap):
    comptime Wrap[T: Copyable & Movable, U: Copyable & Movable] = List[T]
    var v: Int

    def make(self) -> List[Thing]:
        return [Thing(self.v)]

def main():
    print(1)
