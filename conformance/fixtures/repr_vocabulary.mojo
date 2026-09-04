# repr texts shared with current Mojo: scalars wrap in their type name,
# Strings single-quote with backslash escapes, tuples spell
# `Tuple[<element types>](<element reprs>)`, slice descriptors spell their
# keyword bounds, and a user struct's write_repr_to drives `repr`.
from std.format._utils import TypeNames

struct Point(Writable):
    var x: Int
    var y: Int

    def __init__(out self, x: Int, y: Int):
        self.x = x
        self.y = y

    def write_to(self, mut writer: Some[Writer]):
        writer.write("(", self.x, ", ", self.y, ")")

    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write("Point[", TypeNames[Int, Int](), "](", self.x, ", ", self.y, ")")

def main():
    print(repr(7), repr(String("hi")), repr(True), repr(2.5), repr(-1))
    print(repr((0, String("hello"))), repr((1, 2)))
    print(repr(Slice(1, 4)), repr(Slice(1, 4, 2)), repr(Slice(None, 4, None)))
    print(repr(String("a\nb")), repr(String("it's")), repr(String("tab\there")))
    var point = Point(1, 2)
    print(point, repr(point))
