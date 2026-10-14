# Probe: a struct declaring `TrivialRegisterPassable` but not `Copyable` is
# copied explicitly, by `q.copy()` and by `P(copy=q)`.
#
# The pin (2026-10-14) runs it and prints `1` twice:
# `TrivialRegisterPassable` refines `ImplicitlyCopyable`, which refines
# `Copyable`. Mojito rejects it, "type 'P' has no method 'copy'".
# docs/roadmap.md R863. When Mojito runs it, promote this file to
# `assets/ok/` with its manifest rows.
@fieldwise_init
struct P(TrivialRegisterPassable, Writable):
    var a: Int


def main():
    var q = P(1)
    var r = q.copy()
    var s = P(copy=q)
    print(r.a)
    print(s.a)
