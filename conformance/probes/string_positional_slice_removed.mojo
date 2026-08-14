# PROBE (subset-direction check): does the audited head still accept
# positional slicing on the nominal String (`s[1:4]`), or is the keyword
# spelling (`s[byte=a:b]`) the only surface?
#
# Context: nightly §5 enumerates String's strict slices as `byte=`/
# `codepoint=` keyword slices and upstream already rejects bare positional
# `s[i]`, so Mojito rejects positional String slicing with a hint toward the
# keyword forms (subset-safe under match-or-subset either way).
#
# Run:    mojo run string_positional_slice_removed.mojo
#         cargo run -- run conformance/probes/string_positional_slice_removed.mojo
#
# If Mojo rejects: parity confirmed — delete this probe.
# If Mojo accepts: record what it returns (owned String vs StringSlice) and
#   reinstate a positional overload with that exact contract; update
#   parity row types.strings and the tailored hint in
#   src/checker/indexing.rs (infer_slice_subscript).
def main():
    var s = String("hello")
    print(s[1:4])
