# Probe: the same recursion guarded by a runtime `if` instead of a
# `comptime if`.
#
# The pin (2026-10-02) expands `rep[n - 1]` without end (killed after 60 s;
# its instantiation depth is unlimited by default). Mojito prints `6`,
# because its compile-time evaluation runs the erased body with `n` reified
# at run time. docs/roadmap.md 3.77 ledger; closes when the evaluation runs
# a concrete instance (docs/notes/ctfe-request-path.md).
def rep[n: Int]() -> Int:
    if n == 0:
        return 0
    return n + rep[n - 1]()

comptime R = rep[3]()

def main():
    print(R)
