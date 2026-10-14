# Probe: a `TrivialRegisterPassable` struct holds a `String` field.
#
# The pin (2026-10-14) rejects it: "all members of 'TrivialRegisterPassable'
# struct must themselves be 'TrivialRegisterPassable'". Mojito accepts it and
# prints `a`. docs/roadmap.md R860.
@fieldwise_init
struct P(TrivialRegisterPassable):
    var s: String


def main():
    var p = P("a")
    print(p.s)
