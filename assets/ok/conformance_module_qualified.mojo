# A module-qualified trait names the trait in a struct's conformance list
# and a trait's refinement list, as the name `from std.format import
# Writable` binds does.
import std.format as fm


@fieldwise_init
struct Point(Copyable, fm.Writable):
    var x: Int
    var y: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("(", self.x, ", ", self.y, ")")


trait Shown(fm.Writable):
    pass


@fieldwise_init
struct Tag(Shown):
    var name: String

    def write_to(self, mut writer: Some[Writer]):
        writer.write("<", self.name, ">")


def show[T: Shown](value: T):
    print(value)


def main():
    print(Point(1, 2))
    show(Tag("b"))
