# A per-instantiation method clone inherits its checked template's facts when
# the body hands a call through the struct parameter's bound a string literal
# or a named value of a closed non-scalar type, by value
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `bound_dispatch`). The argument is a temporary or a place read where it
# lies, whatever the witness, so an instance records what the template did.
# `Repeat` declares another default than the requirement's, which the checker
# spells at a call through the bound (`checker/bound_defaults.rs`): a string
# literal argument too.
trait Scaler:
    def scale(self, value: Int, factor: Int = 2, label: String = "x") -> String:
        ...


@fieldwise_init
struct Offset(Copyable, Deinitable, Scaler):
    var base: Int

    def scale(self, value: Int, factor: Int = 2, label: String = "x") -> String:
        return label + String(self.base + value * factor)


@fieldwise_init
struct Repeat(Copyable, Deinitable, Scaler):
    var text: String

    def scale(self, value: Int, factor: Int = 2, label: String = "r") -> String:
        return label + self.text * (value * factor)


struct Holder[S: Scaler & Copyable & Deinitable](Movable):
    var s: Self.S
    var tag: String

    def __init__(out self, var s: Self.S):
        self.s = s^
        self.tag = String("t")

    def keyword_literal(self) -> String:
        return self.s.scale(2, label="y")

    def positional_literal(self) -> String:
        return self.s.scale(1, 3, "z")

    def named_local(self) -> String:
        var label = String("w")
        return self.s.scale(1, label=label)

    def named_parameter(self, label: String) -> String:
        return self.s.scale(1, 1, label)

    def field_argument(self) -> String:
        return self.s.scale(1, label=self.tag)

    def spelled_default(self) -> String:
        return self.s.scale(1)


def main():
    var offset = Holder[Offset](Offset(1))
    var repeat = Holder[Repeat](Repeat(String("ab")))
    print(
        offset.keyword_literal(),
        offset.positional_literal(),
        offset.named_local(),
        offset.named_parameter(String("p")),
        offset.field_argument(),
        offset.spelled_default(),
    )
    print(
        repeat.keyword_literal(),
        repeat.positional_literal(),
        repeat.named_local(),
        repeat.named_parameter(String("p")),
        repeat.field_argument(),
        repeat.spelled_default(),
    )
