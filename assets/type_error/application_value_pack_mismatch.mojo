# Two constructions of one `StaticTuple` value are distinct applications, as
# at the pin: the `*elems` initializer's and the `*values` one's.
# expect: type mismatch for variable 'a'
from std.utils import StaticTuple


struct Tag[t: StaticTuple[Int, 2]]:
    def __init__(out self):
        pass


def main():
    var a: Tag[StaticTuple[Int, 2](1, 0)] = Tag[
        StaticTuple[Int, 2].__init__[1, 0]()
    ]()
