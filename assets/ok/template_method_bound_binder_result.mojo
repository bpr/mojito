# A per-instantiation method clone inherits its checked template's facts when
# the body calls through a struct parameter's bound a requirement whose result
# is typed by the requirement's own binder (`docs/notes/instantiation-from-template.md`,
# class MethodBody, feature `bound_dispatch`). The binder is inferred from a
# by-value argument's type, so the result is the argument's type in every
# instance, and its copy, move, or drop is proved again at that type.
trait Merger:
    def echo[T: Copyable](self, other: T) -> T:
        ...


@fieldwise_init
struct Count(Copyable, Merger):
    var n: Int

    def echo[T: Copyable](self, other: T) -> T:
        return other.copy()


@fieldwise_init
struct Word(Copyable, Merger):
    var text: String

    def echo[T: Copyable](self, other: T) -> T:
        return other.copy()


struct Holder[S: Merger & Copyable & Deinitable, W: Writable & Copyable & Deinitable](Movable):
    var s: Self.S
    var w: Self.W

    def __init__(out self, var s: Self.S, var w: Self.W):
        self.s = s^
        self.w = w^

    def echoed_field(self) -> Self.W:
        return self.s.echo(self.w)

    def echoed_local(self) -> Self.W:
        var echoed = self.s.echo(self.w)
        return echoed^

    def echoed_nested(self) -> Self.W:
        return self.s.echo(self.s.echo(self.w))

    def echoed_discarded(self):
        _ = self.s.echo(self.w)

    def echoed_parameter(self, other: Self.W) -> Self.W:
        return self.s.echo(other)


def main():
    var count = Holder[Count, String](Count(2), String("w"))
    var word = Holder[Word, Int](Word(String("ab")), 7)
    count.echoed_discarded()
    word.echoed_discarded()
    print(count.echoed_nested(), word.echoed_nested())
    print(count.echoed_field(), count.echoed_local(), count.echoed_parameter(String("q")))
    print(word.echoed_field(), word.echoed_local(), word.echoed_parameter(9))
