# A `type_of` construction over a value built through a local `comptime`
# alias of its type (`type_of(bits)(BitsType(4))`) in a generic body, the
# shape upstream's `_floor` uses.
def low_bits_cleared(x: SIMD) -> type_of(x):
    var bits = x._to_bits_signed()
    comptime BitsType = type_of(bits)
    bits = bits & type_of(bits)(~((1 << BitsType(4)) - 1))
    return type_of(x)(bits.cast[x.dtype]())


def main():
    print(low_bits_cleared(Int32(1000)), low_bits_cleared(SIMD[DType.int64, 2](37, -37)))
