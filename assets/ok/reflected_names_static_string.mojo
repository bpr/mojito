# `_unqualified_type_name[T]()` and an element of `reflect[T].field_names()`
# are compile-time `StaticString`s, so a parameter list takes either, bound
# by name or spelled in place, over a closed type or a type parameter.
from std.collections.string.string_span import _get_kgen_string
from std.reflection import reflect
from std.reflection.type_info import _unqualified_type_name


struct Holder(Copyable):
    var a: Int
    var b: Int


def closed_name() -> StaticString:
    comptime n = _unqualified_type_name[Holder]()
    return StaticString(_get_kgen_string["closed ", n]())


def inline_name() -> StaticString:
    return StaticString(
        _get_kgen_string["inline ", _unqualified_type_name[Holder]()]()
    )


def symbolic_name[T: AnyType]() -> StaticString:
    comptime n = _unqualified_type_name[T]()
    return StaticString(_get_kgen_string["symbolic ", n]())


def field_name[Parent: AnyType, I: Int]() -> StaticString:
    comptime names = reflect[Parent].field_names()
    comptime name = names[I]
    return StaticString(_get_kgen_string["field ", name]())


def main():
    print(closed_name())
    print(inline_name())
    print(symbolic_name[Holder]())
    print(field_name[Holder, 0](), field_name[Holder, 1]())
