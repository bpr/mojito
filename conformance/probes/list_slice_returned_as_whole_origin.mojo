# PROBE: a contiguous `List` slice returned as a `Span` over the whole list's origin.
#
# The pin rejects it: "cannot implicitly convert 'Span[Int,
# origin_of(xs["element"])]' value to 'Span[Int, origin_of(xs)]'".
# Mojito accepts it and prints [1, 2]: an owned-interior origin converts to
# its base origin.
#
# Observed 2026-10-19 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# When Mojito rejects it, move this file to `assets/type_error/` with its
# `conformance/assets-mojo-errors.tsv` row (R1153).
#
# Run:    mojo run list_slice_returned_as_whole_origin.mojo
#         cargo run -- run conformance/probes/list_slice_returned_as_whole_origin.mojo
def mid(ref xs: List[Int]) -> Span[Int, origin_of(xs)]:
    return xs[1:3]

def main():
    var xs: List[Int] = [0, 1, 2, 3]
    print(mid(xs))
