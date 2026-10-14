# Probe: a `Copyable` struct holds a `TrivialRegisterPassable` field.
#
# The pin (2026-10-14) runs it and prints `2`: `TrivialRegisterPassable`
# refines `ImplicitlyCopyable`, so the field is copyable. Mojito rejects it,
# "type 'Box' for parameter 'Self' does not conform to trait 'Copyable'".
# docs/roadmap.md R861. When Mojito runs it, promote this file to
# `assets/ok/` with its manifest rows.
@fieldwise_init
struct P(TrivialRegisterPassable):
    var x: Int


@fieldwise_init
struct Box(Copyable):
    var p: P


def main():
    var b = Box(P(2))
    print(b.p.x)
