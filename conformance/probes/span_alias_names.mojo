# PROBE (vocabulary pinning): what are the exact mutability-alias spellings
# for the view types at the audited head (`ImmSpan`/`MutSpan`?
# `ImmutableSpan`/`MutableSpan`? none?), and does `StringSlice` still
# resolve as an alias of the canonical string view?
#
# Context: nightly §5 says "with `Imm`/`Mut` aliases" without pinning the
# spellings. Mojito currently ships `Span`/`StringSpan` plus the
# `StringSlice` -> `StringSpan` annotation alias and NO mutability aliases;
# the alias table lives in src/checker/type_resolution.rs so adding the
# probed spellings is a one-line change per name.
#
# Run:    mojo run span_alias_names.mojo
#         cargo run -- run conformance/probes/span_alias_names.mojo
#
# If Mojo accepts `StringSlice` and names the mut aliases: add the probed
#   spellings to the alias table and pin them in parity row types.slices.
# If Mojo rejects `StringSlice`: drop Mojito's compatibility alias.
def main():
    var s = String("hello")
    var v: StringSlice = s[byte=1:4]
    print(v)
