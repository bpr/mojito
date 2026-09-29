# PROBE: an explicit `Tuple(...)` initializer call whose two arguments carry
# the same mutable origin.
#
# **Differs.** The pin rejects it, "aliasing values passed mutably to 'args'
# argument and passed mutably to 'args' argument in 'Tuple[Span[Int,
# origin_of(xs)], Span[Int, origin_of(xs)]]' initializer call". Mojito runs
# no argument aliasing rule on a constructor call, so it prints 3. Filed in
# `docs/roadmap.md` §3 (3.100). When Mojito rejects it, move this file to
# `assets/type_error/` with its ledger row.
#
# Observed 2026-09-29 against the pinned Mojo.
#
# Run:    mojo run initializer_arguments_share_mutable_origin.mojo
def main():
    var xs: List[Int] = [4, 5, 6]
    var t = Tuple(Span(xs), Span(xs))
    print(len(t[0]))
