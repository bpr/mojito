# expect: expected Buf[n], found Buf[--n]
# Unary negation of a symbolic operand is an opaque atom, not a multiplication
# by -1: the pinned Mojo does not equate `-(-n)` with `n`, so the canonical
# form must not either (`docs/notes/param-expr-attributes.md`).
struct Buf[n: Int](Copyable, Movable):
    var value: Int

    def __init__(out self, value: Int):
        self.value = value


def twice[n: Int](var x: Buf[-(-n)]) -> Buf[n]:
    return x^


def main():
    print(1)
