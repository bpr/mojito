# `==` needs an `Equatable` element, which its `where` clause states.
# expect: operator '==' is not defined for
from std.utils import StaticTuple


struct P(ImplicitlyCopyable, RegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x


def main():
    var t = StaticTuple[P, 2](fill=P(1))
    print(t == t)
