# `Self(...)` in a trait's default method constructs the conformer through
# the trait's `__init__` requirement; the pin prints "7", Mojito reports
# "constructing type parameter '_Self' through the `__init__` requirement of
# trait 'Mk'" (docs/roadmap.md R283). Promote it to an assets/ok fixture
# once R283 lands.
trait Mk(Copyable, Movable):
    def __init__(out self, a: Int):
        ...

    def twin(self) -> Self:
        return Self(7)


struct A(Mk):
    var a: Int

    def __init__(out self, a: Int):
        self.a = a


def main():
    print(A(1).twin().a)
