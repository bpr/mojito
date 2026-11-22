# PROBE: a built-in trivial type bound by `TrivialRegisterPassable`.
#
# **Differs.** The pin prints `7 a`: `Pointer` and `StringLiteral` declare
# `TrivialRegisterPassable` upstream. Mojito accepts `keep(p)` but infers
# `keep("a")` at `String`: the checker's `is_trivial_register_passable`
# omits a `StringLiteral[v]` application.
# `docs/roadmap.md` R1502. When Mojito runs it, move this to `assets/ok/`.
def keep[T: TrivialRegisterPassable](x: T) -> T:
    return x


def main():
    var x = 7
    var p = Pointer(to=x)
    print(keep(p)[], keep("a"))
