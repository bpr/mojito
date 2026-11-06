# A `comptime assert` message may be a `StaticString` construction over a
# `_get_kgen_string` concatenation, as upstream's `_constrained_conforms_to`
# spells its message, and a local `comptime` alias of a parameter keeps its
# place in the concatenation's pack wherever it is lifted.
from std.builtin.constrained import _constrained_conforms_to
from std.collections.string.string_span import _get_kgen_string


struct Foo:
    var x: Int


def check[T: AnyType, P: StaticString]():
    comptime q = P
    comptime assert True, StaticString(_get_kgen_string["(", q, ")"]())
    _constrained_conforms_to[
        conforms_to(T, Copyable), Parent=Foo, Element=T, ParentConformsTo=P
    ]()
    comptime m = StaticString(_get_kgen_string["a", P, q]())
    print(m)


def main():
    comptime assert True, StaticString(_get_kgen_string["a", "b"]())
    check[Int, "Copyable"]()
    check[String, "Copyable"]()
