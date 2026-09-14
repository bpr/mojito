# Question: a conditional partial move on one branch joined with a whole
# move on the other, and no reinitialization.
# Upstream (`1.1.0.dev2026082605`) rejects it: "field 'p.a' destroyed out
# of the middle of a value, preventing the overall value from being
# destroyed".
#
# Mojito today runs it (`del 1`, `del 2`): the three-point move lattice
# joins `a: MaybeMoved` under `base: Owned` with `base: Moved` into
# `a: MaybeMoved` under `base: MaybeMoved`, which is also the state of a
# value that is intact or wholly moved, so the exit check sees no hole.
#
# On the fix: promote this file to
# `assets/ownership_error/partial_move_join_imprecision.mojo` with
# `# expect: field 'p.a' destroyed out of the middle of a value`, add its
# `conformance/assets-mojo-errors.tsv` row, and delete the
# `partial-move-join-imprecision` ledger row in docs/roadmap.md.
@fieldwise_init
struct Inner:
    var id: Int
    def __deinit__(deinit self):
        print("del", self.id)

@fieldwise_init
struct Pair:
    var a: Inner
    var b: Inner

def main():
    var flag = True
    var other = False
    var p = Pair(Inner(1), Inner(2))
    if flag:
        if other:
            var x = p.a^
            print(x.id)
    else:
        var q = p^
        print(q.b.id)
