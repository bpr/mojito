# expect: violated constraint
# A call whose argument's `dtype` makes a `where` clause reading that
# argument `False` violates the clause.
def doubled(x: SIMD) -> type_of(x) where x.dtype.is_floating_point():
    return x + x


def main():
    print(doubled(Int32(1)))
