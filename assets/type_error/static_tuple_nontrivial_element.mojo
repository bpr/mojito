# A `StaticTuple` element whose copy runs an initializer fails the
# construction check, with the element type in the message.
# expect: constraint failed: `StaticTuple` element type must have a trivial move/copy constructor and destructor: static_tuple_nontrivial_element.R
from std.utils import StaticTuple


struct R(ImplicitlyCopyable, RegisterPassable):
    var x: Int

    def __init__(out self, x: Int):
        self.x = x

    def __init__(out self, *, copy: Self):
        self.x = copy.x


def main():
    var t = StaticTuple[R, 2](fill=R(1))
    print(t[0].x)
