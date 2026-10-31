# Upstream's `IntLiteral[value: __mlir_type.`!pop.int_literal`]` and
# `FloatLiteral[value: __mlir_type.`!pop.float_literal`]` structs: an
# application over a literal's `value` constructs one, and its members read
# the POP literal attribute through `#pop.int_literal_convert`,
# `#pop<float_to_int_literal>`, and `#pop<float_literal_isa>`.
from std.math import ceil, floor, trunc

comptime V = (7).value
comptime F = (-3.7).value


def g[T: Intable](x: T) -> Int:
    return Int(x)


def main():
    comptime v = (4).value
    print(Int(IntLiteral[v]()) + 1, IntLiteral[(4).value]())
    print(IntLiteral[V](), repr(IntLiteral[V]()), g(IntLiteral[V]()))
    print(ceil(IntLiteral[V]()), floor(IntLiteral[V]()), trunc(IntLiteral[V]()))
    print(Int(IntLiteral[(10**30).value]()))
    print(String(IntLiteral[(-12).value]()))
    print(
        FloatLiteral[F]().__int_literal__(),
        FloatLiteral[F]().__int__(),
        g(FloatLiteral[(2.5).value]()),
    )
    print(
        FloatLiteral[F]().is_nan(),
        FloatLiteral[F]().is_neg_zero(),
        FloatLiteral[F]()._is_normal(),
    )
    print(
        FloatLiteral[(-0.0).value]().is_neg_zero(),
        FloatLiteral[(-0.0).value]()._is_normal(),
        FloatLiteral[(0.0).value]().is_neg_zero(),
    )
