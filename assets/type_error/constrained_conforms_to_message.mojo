# expect: function instantiation of `check` failed: constraint failed: constrained_conforms_to_message.Foo(Copyable) conformance requires constrained_conforms_to_message.Foo(Copyable) conformance, which is not satisfied.
# Upstream's `_constrained_conforms_to` fails for an instance whose element
# does not conform, its message the `StaticString` of a `_get_kgen_string`
# concatenation over the types' reflected names and the trait names.
from std.builtin.constrained import _constrained_conforms_to


struct Foo:
    var x: Int


def check[T: AnyType]():
    _constrained_conforms_to[
        conforms_to(T, Copyable),
        Parent=Foo,
        Element=T,
        ParentConformsTo="Copyable",
    ]()
    print("checked")


def main():
    check[Int]()
    check[Foo]()
