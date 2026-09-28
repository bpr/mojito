# PROBE: a call leaving out a parameter whose default is a construction.
#
# **Differs.** The pin runs it and prints `a`: the default `String("a")` is
# evaluated at the call. Mojito checks it, but its VM refuses the call at run
# time, "vm: non-constant default for parameter 's' of 'f'": a function's
# signature carries a default only as a folded constant or a converting
# construction of one. Filed in `docs/roadmap.md` §3. When Mojito runs it,
# promote this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-28 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run constructed_default_argument.mojo


def f(s: String = String("a")) -> String:
    return s


def main():
    print(f())
