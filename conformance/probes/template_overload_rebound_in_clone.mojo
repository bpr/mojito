# PROBE (divergence): an overloaded call re-ranked in an uncovered instance.
#
# The pinned Mojo binds `pick(kept)` once, while it checks `outer` with `T`
# symbolic, so both lines print 2. Mojito inherits that choice only for an
# instance derived from its checked template
# (`assets/ok/template_overload_binding.mojo`, and with a scalar local
# `assets/ok/template_overload_binding_local.mojo`). This body binds a local
# of the parameter type, which no derivation class of a surviving trait-bound
# template admits, so `outer$Int` still takes the clone check and ranks the
# set again on `Int`.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    2, 2
#   mojito: 1, 2
#
# Run:    mojo run template_overload_rebound_in_clone.mojo
#         cargo run -- run conformance/probes/template_overload_rebound_in_clone.mojo
#
# When answered: a `def` class that admits a local of a parameter type closes
# it, and this file joins `assets/ok/template_overload_binding_local.mojo`.
# The roadmap entry is "A clone that is still checked re-ranks an overloaded
# call" (section 3).
def pick(x: Int) -> Int:
    return 1


def pick[T: Copyable](x: T) -> Int:
    return 2


def outer[T: ImplicitlyCopyable & Deinitable](x: T) -> Int:
    var kept = x
    return pick(kept)


def main():
    print(outer(3))
    print(outer(True))
