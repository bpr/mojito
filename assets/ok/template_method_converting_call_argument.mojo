# A per-instantiation method clone inherits its checked template's facts when
# the body hands a converting argument to a method call
# (`docs/notes/instantiation-from-template.md`, class MethodBody, obligation
# Implicit conversions). Such an argument records its conversion twice: once
# under its own occurrence and once in the call's boundary, which lowering
# reads. The instance selects the constructor again from its own source and
# target types and writes it back into both, so a literal picks one member of
# a closed family and a value of the struct's parameter type reaches the
# constructor clone of a target built over that parameter.
struct Label(Copyable, Deinitable, Movable):
    var count: Int

    @implicit
    def __init__(out self, count: Int):
        self.count = count

    @implicit
    def __init__(out self, flag: Bool):
        self.count = 2 if flag else 3


struct Wrapper[T: Copyable & Deinitable](Copyable, Deinitable, Movable):
    var value: Self.T

    @implicit
    def __init__(out self, value: Self.T):
        self.value = value.copy()


struct Holder[T: Copyable & Deinitable](Deinitable, Movable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def rank(self, label: Label) -> Int:
        return label.count

    def keep(self, box: Wrapper[Self.T]) -> Int:
        return 7

    def numbered(self) -> Int:
        return self.rank(4)

    def flagged(self) -> Int:
        return self.rank(True)

    def boxed(self) -> Int:
        return self.keep(self.item)


def main():
    var number = Holder[Int](5)
    var text = Holder[String](String("hi"))
    print(number.numbered(), number.flagged(), number.boxed())
    print(text.numbered(), text.flagged(), text.boxed())
