# A compile-time string is typed as at the pin: a `[s: String]` value
# parameter is a `String`, a `[s: StringLiteral]` one a `StringLiteral[_]`,
# the join of two different literals the `String` both materialize to, and a
# reflected name (`_unqualified_type_name`, a `field_names()` element) a
# `StaticString`.
from std.reflection import reflect
from std.reflection.type_info import _unqualified_type_name


struct Point:
    var x: Int
    var y: Int


def text[s: String]():
    print(_unqualified_type_name[type_of(s)]())
    var t = s
    t += "!"
    print(t, s.byte_length())


def literal[s: StringLiteral]():
    print(s._is_identical("xy"), s.byte_length())


def main():
    text["hi"]()
    literal["xy"]()
    literal["ab"]()
    comptime m = "a" if True else "bb"
    print(_unqualified_type_name[type_of(m)]())
    print(m)
    var c = m.byte_length() > 1
    var r = "a" if c else "bb"
    print(r)
    comptime n0 = reflect[Point].field_names()[0]
    var s: StaticString = n0
    print(s, s.byte_length())
    comptime u = _unqualified_type_name[Point]()
    var v: StaticString = u
    print(v)
