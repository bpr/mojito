# PROBE (deprecation tracking): does the audited head still ACCEPT the
# transitional `SIMDSize` spelling (with a deprecation warning), or has it
# been removed?
#
# Context: at ae386d1b204, upstream `std/builtin/simd_length.mojo` ships
# `@deprecated("`SIMDSize` is deprecated, use `SIMDLength`.")` — so Mojito
# keeps accepting the alias (a deprecation bridge, never emitted). Re-run
# this probe at every re-pin; Mojito accepts silently and prints 4.
#
# Run:    mojo run simd_size_deprecated_alias.mojo
#         cargo run -- run conformance/probes/simd_size_deprecated_alias.mojo
#
# If Mojo accepts (warning ok): no action — the bridge stands.
# If Mojo rejects: remove Mojito's alias acceptance
#   (src/checker/annotations.rs SIMDSize arm and the src/comptime.rs
#   spellings) and update parity row values.simd / grammar.md.
# (The width argument is explicit because Mojito does not yet infer a value
# parameter from the argument's type — that is a separate, known gap.)
def lane_count[width: SIMDSize](v: SIMD[DType.int, width]) -> Int:
    return width

def main():
    print(lane_count[4](SIMD[DType.int, 4](0)))
