# Question: `w = keep(view_x(w))`, where the free function `view_x` returns a
# view declared under an owned interior of its argument.
# Upstream (`1.1.0.dev2026082605`) rejects it: "aliasing values passed
# immutably to 'v' argument and constructed as a result in 'keep' call".
#
# Mojito runs it and prints `ab`: a free call carries its arguments' origins
# unprojected, so the view's `w.x.<bytes>` origin reads as `w` itself, which
# the rule allows.
#
# On the fix: move this file to `assets/type_error/` with an `# expect:` line
# and its `conformance/assets-mojo-errors.tsv` row, and delete the
# `result-alias-rule-coverage` ledger row in docs/roadmap.md.
@fieldwise_init
struct W(Movable):
    var x: String

def view_x(v: W) -> StringSpan[origin_of(v.x)._get_owned_interior["bytes"]]:
    return v.x.rstrip()

def keep(v: StringSpan) -> W:
    return W(String(v))

def main():
    var w = W(String("ab  "))
    w = keep(view_x(w))
    print(w.x)
