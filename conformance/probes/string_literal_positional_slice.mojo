# PROBE (compatibility-surface check): does the audited head still accept
# positional slicing on a StringLiteral-typed binding, with Python-style
# normalization?
#
# Context: Mojito keeps the builtin literal slice (compiler intrinsic,
# `slice_value` in src/runtime.rs — normalizing, byte-wise, lossy on
# mid-sequence cuts) while the nominal String rejects positional slicing.
# If upstream removed literal slicing too, the intrinsic path should be
# retired with the same keyword-slice hint.
#
# Run:    mojo run string_literal_positional_slice.mojo
#         cargo run -- run conformance/probes/string_literal_positional_slice.mojo
#
# If Mojo accepts (prints `ell` then `olleh`): parity confirmed — delete
#   this probe.
# If Mojo rejects: retire `intrinsic_slice_dispatch`'s StringLiteral arm
#   (src/mir.rs) and flip assets/ok/slice.mojo's literal cases to
#   type_error.
def main():
    var s: StringLiteral = "hello"
    print(s[1:4])
    print(s[::-1])
