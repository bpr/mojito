# A per-instantiation method clone inherits its checked template's facts when
# the body calls a method through the struct parameter's bound and leaves a
# defaulted parameter of the requirement to its default
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `bound_dispatch`). Current Mojo runs the requirement's default; every
# witness declares that default alike, so the one an instance runs is the
# same, and the call records only the omitted slot.
trait Scaler:
    def scale(
        self, value: Int, factor: Int = 2, offset: Int = 0, label: String = "x"
    ) -> String:
        ...


@fieldwise_init
struct Offset(Copyable, Deinitable, Scaler):
    var base: Int

    def scale(
        self, value: Int, factor: Int = 2, offset: Int = 0, label: String = "x"
    ) -> String:
        return label + String(self.base + value * factor + offset)


@fieldwise_init
struct Repeat(Copyable, Deinitable, Scaler):
    var text: String

    def scale(
        self, value: Int, factor: Int = 2, offset: Int = 0, label: String = "x"
    ) -> String:
        return label + self.text * (value * factor + offset)


struct Holder[S: Scaler & Copyable & Deinitable](Movable):
    var s: Self.S

    def __init__(out self, var s: Self.S):
        self.s = s^

    def both_default(self) -> String:
        return self.s.scale(3)

    def one_default(self) -> String:
        return self.s.scale(1, 3)

    def keyword_offset(self) -> String:
        return self.s.scale(2, offset=1)


def main():
    var offset = Holder[Offset](Offset(1))
    var repeat = Holder[Repeat](Repeat(String("ab")))
    print(offset.both_default(), offset.one_default(), offset.keyword_offset())
    print(repeat.both_default(), repeat.one_default(), repeat.keyword_offset())
