# A `type_of` construction over a value built through a local `comptime`
# alias of its type (`type_of(bits)(BitsType(4))`) in a generic body, the
# shape upstream's `_floor` uses. A `type_of` construction over a literal
# (`type_of(y)()` of `comptime y = 2 + 3`) is the literal, materialized
# where an unannotated `var` binds it.
def low_bits_cleared(x: SIMD) -> type_of(x):
    var bits = x._to_bits_signed()
    comptime BitsType = type_of(bits)
    bits = bits & type_of(bits)(~((1 << BitsType(4)) - 1))
    return type_of(x)(bits.cast[x.dtype]())


def main():
    print(low_bits_cleared(Int32(1000)), low_bits_cleared(SIMD[DType.int64, 2](37, -37)))
    comptime y = 2 + 3
    var n = type_of(y)()
    var f = type_of(1.5)()
    print(type_of(y)(), n + 1, f)
