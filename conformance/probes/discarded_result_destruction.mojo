# Question: is a call result that a statement discards destroyed at once?
# Mojo destroys it before the next statement runs (`make()` prints `del tok
# 2` before `after discard`, and `with Mgr():` destroys the unbound
# `__enter__` result before the body runs: `enter`, `del tok 1`, `body`).
#
# Mojito today (2026-09-08, `a79fbdf59f2`): the VM never destroys a
# discarded call result — `del tok 2` is not printed at all, and the
# `with Mgr():` shape leaks the enter result. Both backends agree with each
# other, so the exe differential does not see it.
#
# On the fix: promote this file to an `assets/ok` fixture (parity + scalar
# manifest rows, exe ratchet bump), add a `cases.tsv` `run` row, and delete
# the discarded-result item from the behavioral-divergences task in
# `docs/roadmap.md`.
struct Tok:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del tok", self.n)


struct Mgr:
    def __init__(out self):
        pass

    def __enter__(self) -> Tok:
        print("enter")
        return Tok(1)

    def __exit__(self):
        print("exit")


def make() -> Tok:
    return Tok(2)


def main():
    make()
    print("after discard")
    with Mgr():
        print("body")
    print("done")
