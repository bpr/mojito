# PROBE: the built-in `len` over a runtime pack in a `comptime for` header.
#
# Mojito prints a, 2; the pin rejects `range(len(items))` with "cannot use a
# dynamic value in call argument" and wants `items.__len__()`, which both
# compilers accept (assets/ok/template_pack_elements.mojo). Mojito accepts a
# program the pin rejects, so this is a divergence.
#
# Observed 2026-09-23 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run pack_len_comptime_for_header.mojo
#         cargo run -- run conformance/probes/pack_len_comptime_for_header.mojo
def count[*Ts: Writable](*items: *Ts):
    comptime for i in range(len(items)):
        print(items[i])


def main():
    count("a", 2)
