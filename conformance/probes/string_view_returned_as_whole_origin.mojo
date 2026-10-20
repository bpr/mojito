# PROBE: a `String` view returned as a `StringSlice` over the whole string's origin.
#
# The pin rejects it: "cannot implicitly convert 'StringSpan[origin_of(s["bytes"])]'
# value to 'StringSpan[origin_of(s)]'". Mojito accepts it and prints `ab`:
# an owned-interior origin converts to its base origin.
#
# Observed 2026-10-19 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# When Mojito rejects it, move this file to `assets/type_error/` with its
# `conformance/assets-mojo-errors.tsv` row (R1153).
#
# Run:    mojo run string_view_returned_as_whole_origin.mojo
#         cargo run -- run conformance/probes/string_view_returned_as_whole_origin.mojo
def trimmed(ref s: String) -> StringSlice[origin_of(s)]:
    return s.rstrip()

def main():
    var s = String("ab  ")
    print(trimmed(s))
