# PROBE (yield category + diagnostics): does `for x in span` at the audited
# head yield element references (write-through with `for ref x`) or copies,
# and what is the diagnostic for structurally mutating the source List
# during span iteration? (Mojito: `_SpanIter` yields
# `ref[iterable_origin._get_owned_interior["element"]]` references,
# write-through works on a mutable source, and mutation during iteration
# rejects as "access to 'xs' conflicts with live reference 'sp'" — the
# span's whole-place ctor loan, not the interior-invalidation family.)
#
# Context: nightly §5 follow-up, landed with the §7 pass. Mojito's iterator
# is stdlib/std/span.mojo `_SpanIter` on the origin-parameterized protocol.
#
# Run:    mojo run span_iteration_yield.mojo
#         cargo run -- run conformance/probes/span_iteration_yield.mojo
#
# If Mojo yields copies: flip `_SpanIter.__next__` to a value result and
#   drop the write-through claim from features.md's Types row.
# If Mojo's during-iteration mutation diagnostic differs in family: record
#   the wording in parity row types.slices.
def main():
    var xs = List[Int]()
    xs.append(1)
    xs.append(2)
    var sp = Span(xs)
    for ref x in sp:
        x += 10
    print(xs[0] + xs[1])
