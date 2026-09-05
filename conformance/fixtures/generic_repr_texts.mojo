# The repr family in upstream's text: `List`/`Dict`/`Optional` spell
# `Name[params](fields)`, a user struct builds its own representation with
# upstream's fluent `FormatStruct` chain on a temporary, a `write_to`-only
# struct keeps the field-wise default repr, and `OptionalReg` holds a
# trivial payload.
from std.collections import OptionalReg
from std.format._utils import FormatStruct

@fieldwise_init
struct Point(Writable, Copyable, Movable):
    var x: Int
    var y: Bool

    def write_to(self, mut writer: Some[Writer]):
        FormatStruct(writer, "Point").params(2, "tag").fields(self.x, self.y)


@fieldwise_init
struct Plain(Writable, Copyable, Movable):
    var n: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("plain ", self.n)


def main():
    var xs: List[Int] = [1, 2]
    print(repr(xs))
    var names: List[String] = ["x"]
    print(repr(names))
    var d: Dict[String, Int] = {"a": 1}
    print(repr(d))
    var o = Optional[Int](5)
    print(repr(o))
    var e = Optional[Int]()
    print(repr(e))
    print(Point(3, True))
    print(repr(Plain(4)))
    var a = OptionalReg[Int](5)
    var b = OptionalReg[Int]()
    var c: OptionalReg[Int] = 9
    print(Bool(a), Bool(b), a.value(), c.value(), b.or_else(1), a is None, b is None)
