# expect: requires a surrounding 'try' block
# A prefix operator whose dunder raises is a raising call: a function that
# does not raise cannot apply it outside a `try`.


struct S(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __neg__(self) raises -> S:
        if self.v > 0:
            raise Error("neg")
        return S(-self.v)


def negate(s: S) -> Int:
    var t = -s
    return t.v


def main():
    print(negate(S(-1)))
