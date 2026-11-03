# A bare `StringLiteral` argument is upstream's auto-parameterized
# `StringLiteral[_]`: each call instantiates per literal text, so the
# struct's declared members and its `value` parameter read it.
def same(s: StringLiteral) -> Bool:
    return s._is_identical("abc")


def size(s: StringLiteral) -> Int:
    var x = s.value
    return Int(mlir_value=__mlir_op.`pop.string.size`(x))


def shout(s: StringLiteral) -> String:
    return String(s) + "!"


def main():
    print(same("abc"), same("x"))
    print(size("abc"), size("hello"))
    print(shout("hi"))
    print(StringLiteral._from["xyz"]())
