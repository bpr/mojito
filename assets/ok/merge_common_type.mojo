# Two values merging into one result (a conditional expression's arms, a
# display's elements, `and`/`or`'s operands) take upstream's
# `findCommonType`: equal types, then the type one value converts to,
# then a shared materialization target. A run-time merge materializes a
# literal result, a dictionary display binds `K` and `V` from its first
# entry, and an ambiguous `and`/`or` merge falls back to `Bool`.
from std.reflection.type_info import _unqualified_type_name


def k(x: Int8):
    print(x)


def main() raises:
    var c = True
    var d = False
    var a = 1 if c else 1.0
    var b = 1 if d else Float64(2)
    var e = 1 if c else (0.5 if d else 0.25)
    print(a, b, e)
    var n = 1 if c else 2
    print(n, _unqualified_type_name[type_of(n)]())
    var ones = [1, 1]
    print(len(ones), _unqualified_type_name[type_of(ones)]())
    var optionals = [1, Optional[Int](2)]
    print(optionals[0].value(), optionals[1].value())
    var by_optional = {1: Optional[Int](2), 3: 4}
    print(by_optional[3].value())
    var by_float = {1: 2.5, 3: 4}
    print(by_float[3])
    print(0 or 2.5, 1 or 2)
    comptime D = 1 if True else 1
    k(D)
