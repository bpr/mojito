# `not x` calls `x.__bool__()`, so a raising one needs a raising context.
# expect: call to raising method '__bool__' requires a surrounding 'try' block or enclosing function to declare 'raises'


struct R(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __bool__(self) raises -> Bool:
        return self.v != 0


def test(r: R) -> Bool:
    return not r


def main():
    print(test(R(2)))
