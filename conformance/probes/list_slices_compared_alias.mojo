# PROBE: `!=` between two mutable slices of one list.
#
# The pin rejects it: "aliasing values passed mutably to 'self' argument
# and passed mutably to 'rhs' argument in '__ne__' call" (`Span.__ne__`,
# unlike `__eq__`, is not `@__unsafe_nested_origins_read_only`). Mojito
# accepts it and prints True: `rhs: Span[Self.T, _]` has an unbound origin,
# which carries no origin into argument exclusivity (R1156).
#
# Observed 2026-10-19 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# When Mojito rejects it, move this file to `assets/type_error/` with its
# `conformance/assets-mojo-errors.tsv` row.
#
# Run:    mojo run list_slices_compared_alias.mojo
#         cargo run -- run conformance/probes/list_slices_compared_alias.mojo
def main():
    var xs: List[Int] = [1, 2, 3]
    print(xs[0:2] != xs[1:3])
