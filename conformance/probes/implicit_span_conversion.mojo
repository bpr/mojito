# PROBE (temporary-origin inference shape): which argument spellings convert
# implicitly to a `Span` parameter at the audited head, and with what
# resulting mutability? (1) a named `List` variable (Mojito: converts, the
# temporary borrowing the list — this file); (2) a bare list literal
# (upstream deleted `Span(...)` wrappers around literals in 7852cfb; Mojito
# types the literal as fixed-size `Array` and rejects — a recorded subset
# gap, work around by binding to a `List` first); (3) can the callee write
# through the converted span (Mojito's `Span[Int]` parameter spelling keeps
# a parametric mut, so writes reject)?
#
# Context: nightly §7 temporary-origin inference. Mojito's conversion path:
# the @implicit `ref [origin]` Span constructor plus the
# BorrowConversionSource fact (src/checker.rs implicit_conversion_target).
#
# Run:    mojo run implicit_span_conversion.mojo
#         cargo run -- run conformance/probes/implicit_span_conversion.mojo
#
# If Mojo converts literals: close the Array-literal gap (either a
#   Span-from-Array @implicit constructor — mind the stdlib generic
#   multi-__init__ overload-key landmine — or expected-type-driven literal
#   typing) and update the features.md Types row.
# If Mojo's converted span is writable from a mutable source: probe the
#   parameter spelling that requests it and extend the mut solving.
def total(s: Span[Int]) -> Int:
    var acc = 0
    var i = 0
    while i < len(s):
        acc += s[i]
        i += 1
    return acc

def main():
    var xs = List[Int]()
    xs.append(10)
    xs.append(20)
    print(total(xs))
