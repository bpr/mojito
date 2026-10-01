# PROBE: a list literal as the default of a `List` parameter.
#
# **Differs.** The pin prints `3`. Mojito rejects the declaration with
# "type mismatch for default value of 'xs': expected List[Int], found
# Array[Int, 3]": the default is typed without its parameter's annotation
# as context. Filed in `docs/roadmap.md` §3. When `run` prints `3`, promote
# this file to `assets/ok/` with its manifest rows.
#
# Observed 2026-09-30 against `Mojo 1.2.0.dev2026092105 (e9569894)`.
#
# Run:    mojo run list_literal_default_argument.mojo


def lst(xs: List[Int] = [1, 2, 3]) -> Int:
    return len(xs)


def main():
    print(lst())
