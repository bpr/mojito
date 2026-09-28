# PROBE (subset gap): a witness whose default differs from its requirement's.
#
# The pinned Mojo runs the requirement's default through the bound and prints
# `11`. Mojito rejects the program: an instance would run the witness's own
# default, so conformance demands the two spelled alike.
trait Scaler:
    def scale(self, value: Int, factor: Int = 2) -> Int:
        ...


struct Twice(Copyable, Movable, Scaler):
    var base: Int

    def __init__(out self, base: Int):
        self.base = base

    def scale(self, value: Int, factor: Int = 7) -> Int:
        return self.base + value * factor


struct Holder[S: Scaler & Copyable & Deinitable](Movable):
    var s: Self.S

    def __init__(out self, var s: Self.S):
        self.s = s^

    def run(self) -> Int:
        return self.s.scale(5)


def main():
    var h = Holder[Twice](Twice(1))
    print(h.run())
