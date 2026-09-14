# Question: a piece of `s.split(" ")` passed to a call assigned back to `s`.
# Upstream (`1.1.0.dev2026082605`) rejects it: "aliasing values passed
# immutably to 'args' argument and constructed as a result in 'String'
# initializer call". Its `split` returns views at
# `origin_of(self)._get_owned_interior["bytes"]`, so `parts[0]` still
# borrows `s`'s bytes.
#
# Mojito runs it and prints `a`: its `String.split` returns `List[String]`,
# owned copies that borrow nothing.
#
# On the fix: move this file to
# `assets/type_error/split_piece_assigned_over_source.mojo` with its
# `conformance/assets-mojo-errors.tsv` row, and delete the
# `split-returns-owned-strings` ledger row in docs/roadmap.md.
def main():
    var s = String("a b")
    var parts = s.split(" ")
    s = String(parts[0])
    print(s)
