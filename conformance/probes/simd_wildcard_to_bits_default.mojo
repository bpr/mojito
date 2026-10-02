# PROBE: `to_bits()` with its defaulted target on the wildcard vector
# parameter `SIMD[_, _]`, whose dtype is symbolic in the body.
#
# The pin accepts this program; Mojito rejects the parameter annotation with
# "not a valid SIMD element type: a non-DType argument", since the `SIMD[_, _]`
# desugar runs over struct methods only (docs/roadmap.md 3.78,
# `wildcard-vector-parameter-on-a-def`). Inside a `Hasher`'s
# `_update_with_simd`, where the desugar applies, Mojito accepts the same
# `to_bits()` and its leaf clones derive
# (`assets/ok/template_method_simd_leaf_default_bits.mojo`), as does an
# explicit `def bits[dt: DType, w: Int](value: SIMD[dt, w])`
# (`assets/ok/simd_to_bits_default_symbolic.mojo`).
def bits(value: SIMD[_, _]) -> UInt64:
    return value.to_bits().cast[DType.uint64]().reduce_add()


def main():
    print(bits(Int(40)), bits(UInt8(3)), bits(SIMD[DType.int16, 2](1, -1)))
