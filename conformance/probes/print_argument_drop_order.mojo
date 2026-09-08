# Question: when a struct's last use is a field read passed to `print`, is
# the struct destroyed before or after the line prints? Mojo prints the line
# first (`use 1`, then `del tok 1`): the argument is read, the call runs,
# and the owner dies after its last use completes.
#
# Mojito today (2026-09-08, `a79fbdf59f2`): the VM reads the field into a
# register, destroys the owner (its last use), then runs the print — `del
# tok 1` precedes `use 1`. Every `__deinit__`-printing fixture that reads a
# field into `print` is written around this. Both backends agree with each
# other, so the exe differential does not see it.
#
# On the fix: promote this file to an `assets/ok` fixture (parity + scalar
# manifest rows, exe ratchet bump), add a `cases.tsv` `run` row, and delete
# the print-argument item from the behavioral-divergences task in
# `docs/roadmap.md`.
struct Tok:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del tok", self.n)


def main():
    var t = Tok(1)
    print("use", t.n)
    print("mid")
