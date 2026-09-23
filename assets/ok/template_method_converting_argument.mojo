# A per-instantiation method clone inherits its checked template's facts when
# the body binds a value that converts through an `@implicit` constructor
# (`docs/notes/instantiation-from-template.md`, class MethodBody, obligation
# Implicit conversions). The constructor is selected from the source and the
# target type alone, so the template retains which selection to repeat rather
# than which constructor it found, and the instance runs the selection again
# at its own types: one source picks `Label.__init__$ov$Int` and the other
# `Label.__init__$ov$Bool` out of the same family. A conversion whose target
# is built over the struct's parameter is still refused, at the binding
# obligation rather than here.
struct Label(Copyable, Deinitable, Movable, Writable):
    var text: String

    @implicit
    def __init__(out self, count: Int):
        self.text = String("count ") + String(count)

    @implicit
    def __init__(out self, flag: Bool):
        self.text = String("flag ") + String(flag)

    def write_to(self, mut writer: Some[Writer]):
        writer.write(self.text)


struct Holder[T: Copyable & Deinitable & Writable](Deinitable, Movable, Writable):
    var item: Self.T

    def __init__(out self, var item: Self.T):
        self.item = item^

    def write_to(self, mut writer: Some[Writer]):
        var numbered: Label = 4
        var flagged: Label = True
        writer.write(numbered, " ", flagged, " ", self.item)


def main():
    var number = Holder[Int](5)
    print(number)
    var text = Holder[String](String("hi"))
    print(text)
