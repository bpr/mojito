# PROBE (syntax + result-type pinning): is `s[byte=a:b]` the accepted
# upstream spelling for unit-explicit String slicing, and does it return a
# borrowed StringSlice/StringSpan view (not an owned String)?
#
# Context: Mojito parses `name=a:b` bracket arguments as keyword slices
# binding keyword-only slice-descriptor `__getitem__` parameters, and
# String's byte/codepoint keyword slices return borrowed `StringSpan`
# views. The spelling and the view result are pinned from nightly §5's
# description, not from a compiled example.
#
# Run:    mojo run keyword_slice_syntax_shape.mojo
#         cargo run -- run conformance/probes/keyword_slice_syntax_shape.mojo
#
# If Mojo accepts and prints `ell 3`: parity confirmed — delete this probe.
# If Mojo rejects the spelling: record the accepted surface (method calls,
#   different keyword names, …), retarget the parser/checker keyword-slice
#   form, and update grammar.md + parity row types.slices.
def main():
    var s = String("hello")
    var v = s[byte=1:4]
    print(v, len(v))
