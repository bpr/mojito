# Probe: may a compile-time evaluation write a `t"…"` literal or a `Tuple`?
#
# The pin (2026-09-21) prints `n=3` and `(3, 2)`. Mojito stops with
# "TString.write_to: unspecialized type-keyed method": the evaluation's
# subprogram carries the variadic struct's `comptime for` member as its
# template stub (R486).


def label(n: Int) -> String:
    return String(t"n={n}")


def pair(n: Int) -> String:
    return String((n, 2))


def main():
    comptime s = label(3)
    comptime p = pair(3)
    print(s)
    print(p)
