# PROBE (re-probe): a pack-keyed template keeps the clone check.
#
# Both compilers print 1, two, 3.5. A body keyed on a variadic pack is
# validated from its template (`templates.validated_keyed` in `--timings`),
# but its certificate is never complete — its facts name an element no
# instance has fixed — so every instance is still checked as a clone.
# Re-run whenever discovery scheduling or fallback changes.
#
# Observed 2026-09-20 against `Mojo 1.1.0.dev2026082605 (dd957314)`.
#
# Run:    mojo run template_fallback_pack.mojo
#         cargo run -- run conformance/probes/template_fallback_pack.mojo
def show[*Ts: Writable](*values: *Ts):
    comptime for i in range(values.__len__()):
        print(values[i])


def main():
    show(1, "two", 3.5)
