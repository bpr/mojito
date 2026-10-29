# A `!kgen.string` has a run-time form: a string literal's `.value` and a
# `!kgen.string` binder are values a variable, an argument, and a `List`
# hold, and `pop.string.size` reads one at run time, as `StaticString(...)`
# and `get_static_string` do.
from std.builtin.enum_like import KGENString
from std.collections.string.string_span import get_static_string


def size(x: KGENString) -> Int:
    return Int(SIMDLength(mlir_value=__mlir_op.`pop.string.size`(x)))


def view[s: KGENString]() -> StaticString:
    var x = s
    return StaticString(x)


def names[*ns: KGENString]():
    var l = ParameterList[ns.values]()
    print(len(l), StaticString(l[0]), StaticString(l[1]))


def main():
    var x = "héllo".value
    var y = x
    print(size(x), size(y), size("".value))
    var l = List[KGENString]()
    l.append("a".value)
    l.append(x)
    print(len(l), StaticString(l[1]))
    print(view["ab".value]())
    print(get_static_string["ab", "cd"]())
    names["one".value, "three".value]()
