# PROBE: `to_bits()` with its defaulted target on the wildcard vector
# parameter `SIMD[_, _]`, whose dtype is symbolic in the body.
#
# The pin accepts this program; Mojito rejects the parameter annotation with
# "not a valid SIMD element type: a non-DType argument", since the `SIMD[_, _]`
# desugar runs over struct methods only (docs/roadmap.md 3.81,
# `wildcard-vector-parameter-on-a-def`). Inside a `Hasher`'s
# `_update_with_simd`, where the desugar applies, Mojito accepts the same
# `to_bits()` but source validation ends without a verdict on it, so the
# method's leaf clones keep the clone check (docs/roadmap.md 1.14).
def bits(value: SIMD[_, _]) -> UInt64:
    return value.to_bits().cast[DType.uint64]().reduce_add()


def main():
    print(bits(Int(40)), bits(UInt8(3)), bits(SIMD[DType.int16, 2](1, -1)))
