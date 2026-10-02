# PROBE: a method's own `*Ts` pack collecting a `Span`.
#
# **Differs.** The pin prints "ok". Mojito rejects the call with "'Span[_]'
# is not concrete; use '[]' to bind missing parameters", though the same
# pack on a module `def` takes the span. Filed in `docs/roadmap.md` §3
# (3.97). When it runs, promote it to `assets/ok`; the pin also rejects
# `S().show(Span(xs), Span(xs))` over this `var xs`, which Mojito must then
# judge too.
#
# Observed 2026-09-29 against the pinned Mojo.
#
# Run:    mojo run method_pack_span_argument.mojo
struct S:
    def __init__(out self):
        pass

    def show[*Ts: Copyable](self, *args: *Ts):
        pass

def main():
    var xs: List[Int] = [4, 5, 6]
    S().show(Span(xs), 1)
    print("ok")
