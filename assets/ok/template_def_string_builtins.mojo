# A runtime `def` calling a string-making checker builtin keeps the
# template's facts, as a generic struct's method does
# (`assets/ok/template_method_string_builtins.mojo`): `repr(kept)` reads its
# argument where it lies and wraps its string as the nominal `String`, and
# `_unqualified_type_name[T]()` records the binder's spelling, which each
# instance re-renders from its own substituted type.
from std.reflection.type_info import _unqualified_type_name


def shown[T: Writable & ImplicitlyCopyable & Deinitable](x: T) -> Int:
    var kept = x
    var r = repr(kept)
    print(r, _unqualified_type_name[T]())
    return 1


def main():
    print(shown[Int](7), shown[String]("s"))
