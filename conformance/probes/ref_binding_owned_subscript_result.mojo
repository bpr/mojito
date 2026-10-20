# PROBE: a `ref` binding over a subscript whose `__getitem__` returns an
# owned value binds the materialized result, as a `ref` over any other
# owned call result does.
#
# The pin runs it and prints:
#   n0
#
# Observed 2026-10-20 against `Mojo 1.2.0.dev2026101705 (46f49943)`.
# Mojito: fails MIR verification ("dynamic element projection requires
# checked indexed storage, got Names") (R1221).
# When Mojito runs it, promote this file to `assets/ok/` with its manifest rows.
#
# Run:    mojo run ref_binding_owned_subscript_result.mojo
#         cargo run -- run conformance/probes/ref_binding_owned_subscript_result.mojo
struct Names(Sized, Copyable):
    def __init__(out self):
        pass

    def __len__(self) -> Int:
        return 2

    def __getitem__(self, i: Int) -> String:
        return String("n") + String(i)


def main():
    var ns = Names()
    ref r = ns[0]
    print(r)
