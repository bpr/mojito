# PROBE: two mutable views of one list passed to `Span[Int, _]` parameters.
#
# The pin rejects it: "aliasing values passed mutably to 'a' argument and
# passed mutably to 'b' argument in 'f' call". Mojito accepts it and prints
# 6: an argument bound to a parameter whose origin is unbound (`_`) carries
# no origin into argument exclusivity (R1156).
#
# Observed 2026-10-19 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# When Mojito rejects it, move this file to `assets/type_error/` with its
# `conformance/assets-mojo-errors.tsv` row.
#
# Run:    mojo run unbound_origin_arguments_alias.mojo
#         cargo run -- run conformance/probes/unbound_origin_arguments_alias.mojo
def f(a: Span[Int, _], b: Span[Int, _]) -> Int:
    return len(a) + len(b)

def main():
    var xs: List[Int] = [1, 2, 3]
    print(f(Span(xs), Span(xs)))
