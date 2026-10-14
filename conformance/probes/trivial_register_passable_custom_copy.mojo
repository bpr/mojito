# Probe: a `TrivialRegisterPassable` struct declares a copy constructor.
#
# The pin (2026-10-14) rejects it: "trivial types must not declare explicit
# copy constructors; they are trivially copyable". Mojito accepts it and
# prints `copy` then `1`. Its siblings
# `trivial_register_passable_custom_del.mojo` and
# `trivial_register_passable_non_trivial_field.mojo` pin the trait's other two
# requirements. docs/roadmap.md R860.
struct P(TrivialRegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __init__(out self, *, copy: Self):
        print("copy")
        self.x = copy.x


def show(var p: P):
    print(p.x)


def main():
    var p = P(1)
    show(p)
