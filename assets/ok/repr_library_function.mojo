# `repr` is upstream's library function (`std/format/repr.mojo`): a
# `String()` the value's `write_repr_to` writes into, or `"None"` for
# `None`. It reaches a struct's own or reflected member, a scalar's, and an
# intrinsic `DType`'s, and the prelude, `std.format`, and a generic caller
# all name the same function.
from std.format import repr as format_repr


@fieldwise_init
struct P(Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("P(", self.x, ")")


@fieldwise_init
struct Q(Writable):
    var x: Int

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Q")

    def write_repr_to(self, mut writer: Some[Writer]):
        writer.write("Q<", self.x, ">")


def show[T: Writable](x: T) -> String:
    return repr(x)


def main():
    print(repr(P(3)), repr(Q(4)))
    print(repr(42), repr(7.5), repr(True))
    print(repr("hi"), repr(String("it's")))
    print(repr((1, "a")))
    print(repr(None))
    print(repr(DType.int32))
    print(format_repr(P(5)), show(Q(6)), show(DType.float64))
    var text = String()
    DType.uint8.write_repr_to(text)
    print(text)
