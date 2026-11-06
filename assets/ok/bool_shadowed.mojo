# `Bool` is a prelude name, not a keyword: a program's own `struct Bool`
# shadows `std.builtin.bool.Bool` in an annotation, a parameter, a result,
# and a construction alike.
struct Bool(Copyable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def bumped(b: Bool) -> Bool:
    return Bool(b.x + 1)


def main():
    var b: Bool = Bool(3)
    print(b.x, bumped(b).x, 1 < 2)
