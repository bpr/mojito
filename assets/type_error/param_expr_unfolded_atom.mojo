# expect: expected Buf[8 // 2], found Buf[4]
# Replacement re-folds the polynomial part of a type argument and leaves an
# opaque atom unfolded, as the pinned Mojo does: `Buf[n // 2]` at `n = 8` is
# `Buf[8 // 2]`, which is not `Buf[4]` (`docs/notes/param-expr-attributes.md`).
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def halve[n: Int](var x: Buf[n // 2]) -> Buf[n // 2]:
    return x^


def main():
    print(halve[8](Buf[4](7)).value)
