# Question: a temporary plain-origin `Span(xs)` passed to a call assigned
# back to `xs`.
# Upstream (`1.1.0.dev2026082605`) runs it: `1`. The span borrows `xs`
# itself, not an owned interior, and the temporary ends at the call.
#
# Mojito rejects it in the ownership analysis: "access to 'xs' conflicts with
# live reference 'xs'". The temporary argument's anchor now ends before the
# store, as for `s = String(StringSpan(s))`, but the assigned `List[Int]`
# result still records a loan on `xs`, so the store into `xs` conflicts with
# its own new value.
#
# On the fix: promote this file to
# `assets/ok/assign_plain_view_argument_over_list.mojo` with its manifest
# rows, and delete the `assign-plain-span-argument-over-list` ledger row in
# docs/roadmap.md.
def rebuild(v: Span[Int, _]) -> List[Int]:
    return [v[0], v[0]]

def main():
    var xs: List[Int] = [1, 2]
    xs = rebuild(Span(xs))
    print(xs[1])
