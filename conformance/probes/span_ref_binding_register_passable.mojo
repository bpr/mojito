# PROBE: a `Span` is register-passable, so a `ref` binding cannot name one that is an rvalue: `ref r = values[1:]` and the starred `case [f, *ref rr]` are rejected.
#
# The pin rejects it:
#   error: value of type 'Span[Int, origin_of(values["element"])]' doesn't have a memory origin in 'ref' binding
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: rejects the `*ref rr` case only for an unrelated loan conflict
# (R1082); without it, runs the `ref r` binding and prints `2`, because the
# bundled `Span` does not declare `TrivialRegisterPassable`.
# When Mojito rejects it with the pin's text, move this file to
# `assets/type_error/` with its errors-ledger row.
#
# Run:    mojo run span_ref_binding_register_passable.mojo
#         cargo run -- run conformance/probes/span_ref_binding_register_passable.mojo
def main():
    var values: List[Int] = [1, 2, 3]
    ref r = values[1:]
    print(len(r))
    match values:
        case [f, *ref rr]:
            print(f, len(rr))
        case _:
            pass
