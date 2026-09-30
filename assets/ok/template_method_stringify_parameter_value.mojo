# A per-instantiation method clone inherits its checked template's facts when
# the body converts a named value of a struct parameter's type with
# `String(value)` (`docs/notes/instantiation-from-template.md`, class
# MethodBody, feature `stringify`). The builtin writes a numeric or `Bool`
# value itself and any other value through its `Writable` conformance, read
# where it lies, so each instance settles that at its own type.
@fieldwise_init
struct Tag(Copyable, Writable):
    var id: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("#", self.id)


struct Holder[W: Writable & Copyable & Deinitable](Movable):
    var w: Self.W

    def __init__(out self, var w: Self.W):
        self.w = w^

    def field(self) -> String:
        return String(self.w)

    def local(self) -> String:
        var w = self.w.copy()
        return String(w)

    def parameter(self, other: Self.W) -> String:
        return String(other)

    def stored(self) -> Int:
        var text = String(self.w)
        return text.byte_length()


def main():
    var count = Holder[Int](3)
    var flag = Holder[Bool](True)
    var word = Holder[String](String("ab"))
    var tag = Holder[Tag](Tag(7))
    print(count.field(), count.local(), count.parameter(4), count.stored())
    print(flag.field(), flag.local(), flag.parameter(False), flag.stored())
    print(word.field(), word.local(), word.parameter(String("c")), word.stored())
    print(tag.field(), tag.local(), tag.parameter(Tag(12)), tag.stored())
