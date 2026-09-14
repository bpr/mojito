# Question: rebinding a view local to a view over another origin.
# Upstream (`1.1.0.dev2026082605`) rejects it: "cannot implicitly convert
# 'P[origin_of(ys)]' value to 'P[origin_of(xs)]'" — the origin is part of
# the struct's identity.
#
# Mojito today runs it (`9`): origin arguments are erased from checked
# identity, so both constructions have the type `P`.
#
# On the fix: promote this file to
# `assets/type_error/rebound_view_origin_identity.mojo` with its
# `conformance/assets-mojo-errors.tsv` row, and delete this bullet from
# the `erased-origin-parameter` ledger row in docs/roadmap.md.
@fieldwise_init
struct P[m: Bool, //, o: Origin[mut=m]]:
    var src: Pointer[List[Int], Self.o]

def main():
    var xs = List[Int]()
    xs.append(7)
    var ys = List[Int]()
    ys.append(1)
    var p = P(Pointer(to=xs))
    p = P(Pointer(to=ys))
    p.src[][0] = 9
    print(ys[0])
