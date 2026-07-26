trait HasElement:
    comptime Element: AnyType

trait HasCopyableElement:
    comptime Element: Copyable

trait Collection(HasElement, HasCopyableElement):
    def size(self) -> Int: ...

@fieldwise_init
struct IntCollection(Collection):
    comptime Element = Int
    var value: Int

    def size(self) -> Int:
        return 1

@fieldwise_init
struct Point(Writable):
    var x: Int
    var y: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("(", self.x, ", ", self.y, ")")

    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write("Point[x=", self.x, ", y=", self.y, "]")

@fieldwise_init
struct Wrapper[T: Copyable & ImplicitlyDeletable](
    Writable where conforms_to(T, Writable)
):
    var value: Self.T

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Wrapper")

def main():
    var values = [3, 7]
    var point = Point(2, 5)
    print(values[1])
    print(point)
    print(repr(point))
    print("{} {!r}".format(point, point))
    print(Wrapper[Int](9))
