# PROBE (divergence): a place passed to a `ref` parameter and again into a pack.
#
# The pin infers the `ref` parameter mutable from the mutable place, and a pack
# element is held by reference, so it reports "aliasing values passed mutably to
# 'b' argument and passed immutably to 'rest' argument". Mojito accepts the
# call and prints 1. With a regular `c: Int` in place of the pack both accept
# it: a trivial read parameter takes a copy.
#
# Observed 2026-09-19 against `Mojo 1.1.0.dev2026082605 (dd957314)`:
#   pin:    error: aliasing values passed mutably to 'b' argument ...
#   mojito: prints 1
#
# Run:    mojo run ref_argument_aliases_pack_element.mojo
#         cargo run -- run conformance/probes/ref_argument_aliases_pack_element.mojo
#
# When fixed: move this file to the error folder that owns the aliasing check,
# with its `conformance/assets-mojo-errors.tsv` row.
def r[*Ts: Writable](ref b: Int, *rest: *Ts) -> Int:
    return b


def main():
    var x = 1
    print(r(x, x))
