# Question: a `List` slice passed to a call assigned back to the list.
# Upstream (`1.1.0.dev2026082605`) rejects it: "aliasing values passed
# immutably to 'v' argument and constructed as a result in 'rebuild' call".
# Its `List.__getitem__(ref self, slice: ContiguousSlice)` returns
# `Span[Self.T, Self._InteriorOrigin[origin_of(self)]]`, a view of the
# list's owned elements.
#
# Mojito runs it and prints `1`: its contiguous `List` slice returns `Self`,
# a copy that borrows nothing.
#
# On the fix: move this file to
# `assets/type_error/list_slice_assigned_over_source.mojo` with its
# `conformance/assets-mojo-errors.tsv` row, and drop this probe from the
# `contiguous-slice-result` ledger row in docs/roadmap.md.
def rebuild(v: Span[Int, _]) -> List[Int]:
    return [v[0], v[0]]

def main():
    var xs: List[Int] = [1, 2]
    xs = rebuild(xs[0:1])
    print(xs[1])
