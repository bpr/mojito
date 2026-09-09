# Question: inside a `__deinit__` (or any `deinit self`) body, is each field
# of `self` destroyed at that field's own last use? Mojo destroys them
# field by field: a field the body never reads dies at entry, a field it
# reads dies right after that read (`del inner 1`, `del outer start`,
# `uses 2`, `del inner 2`, `del outer end`, `mid`).
#
# Mojito today (2026-09-08, `a79fbdf59f2`): the receiver's residual fields
# are consumed together at `self`'s last use — `del outer start`, `uses 2`,
# `del inner 1`, `del inner 2`, `del outer end`, `mid`. Per-field liveness
# for the consuming receiver is the missing piece (drop elaboration is
# variable-granular). Both backends agree with each other, so the exe
# differential does not see it. An unused receiver (every field dying at
# entry) already matches: `assets/ok/deinit_param_destruction_timing.mojo`.
#
# On the fix: promote this file to an `assets/ok` fixture (parity + scalar
# manifest rows, exe ratchet bump), add a `cases.tsv` `run` row, and delete
# the deinit-body item from the behavioral-divergences task in
# `docs/roadmap.md`.
struct Inner:
    var id: Int

    def __init__(out self, id: Int):
        self.id = id

    def __deinit__(deinit self):
        print("del inner", self.id)


struct Outer:
    var a: Inner
    var b: Inner

    def __init__(out self):
        self.a = Inner(1)
        self.b = Inner(2)

    def __deinit__(deinit self):
        print("del outer start")
        print("uses", self.b.id)
        print("del outer end")


def main():
    var o = Outer()
    print("mid")
