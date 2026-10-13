# expect: invalid call to 'Dict': violated constraint
# A dict display is `Dict`'s `__dict_literal__` constructor, whose `where`
# clause requires `Copyable` keys and values, so a display over a value
# that is only `Movable` is rejected at the display, as the spelled
# constructor call is.
struct M(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v


def main():
    var d = {1: M(1)}
    print(len(d))
