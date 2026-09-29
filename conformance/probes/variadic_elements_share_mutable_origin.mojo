# PROBE: two elements of one `*args` pack, or of one tuple literal, carrying
# the same mutable origin.
#
# **Differs.** The pin rejects both calls, "aliasing values passed mutably
# to 'args' argument and passed mutably to 'args' argument": each collected
# element is an argument of its own. Mojito's argument exclusivity rule
# judges the regular parameter slots and the receiver only, so it runs this
# and prints 3. Filed in `docs/roadmap.md` §3. When Mojito rejects it, move
# this file to `assets/type_error/` with its ledger row.
#
# Observed 2026-09-28 against the pinned Mojo.
#
# Run:    mojo run variadic_elements_share_mutable_origin.mojo
def show[*Ts: Copyable](*args: *Ts):
    pass

def main():
    var xs: List[Int] = [4, 5, 6]
    show(Span(xs), Span(xs))
    var t = (Span(xs), Span(xs))
    print(len(t[0]))
