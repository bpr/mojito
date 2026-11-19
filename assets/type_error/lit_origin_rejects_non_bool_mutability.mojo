# expect: expected 'i1' (a Bool's '_mlir_value')
# A `!lit.origin<mut>`'s mutability operand is a `Bool`'s `_mlir_value`, as
# upstream's `_lit_origin_type_of_mut` spells it: an `Int`'s is rejected at
# the alias's declaration, as at the pin.
comptime _L[n: Int] = __mlir_type[`!lit.origin<`, n._mlir_value, `>`]


def main():
    print(1)
