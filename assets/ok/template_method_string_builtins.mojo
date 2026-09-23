# A per-instantiation method clone inherits its checked template's facts when
# the body calls a string-making checker builtin
# (`docs/notes/instantiation-from-template.md`, class MethodBody, feature
# `string_builtins`). `_unqualified_type_name[Self.T]()` records one type's
# spelling, which the instance re-renders from its own substituted type rather
# than inheriting the template's wording, and `repr(self.item)` reads its
# argument where it lies and wraps its compile-time string result as the
# nominal `String` — a conversion whose constructor no instance changes,
# republished once the instance proves the argument is still `Writable`.
from std.reflection.type_info import _unqualified_type_name


struct Tagged[T: AnyType](
    Deinitable where conforms_to(T, Deinitable),
    Movable where conforms_to(T, Movable),
    Writable where conforms_to(T, Writable),
):
    var item: Self.T

    def __init__(out self, var item: Self.T) where conforms_to(Self.T, Movable):
        self.item = item^

    def write_to(self, mut writer: Some[Writer]) where conforms_to(Self.T, Writable):
        writer.write("Tagged[", _unqualified_type_name[Self.T](), "](", repr(self.item), ")")


def main():
    var number = Tagged[Int](7)
    print(number)
    var text = Tagged[String](String("hi"))
    print(text)
