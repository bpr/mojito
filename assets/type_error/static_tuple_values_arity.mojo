# `__init__[*values]` with fewer values than the size fails its arity check.
# expect: constraint failed: mismatch in the number of elements
from std.utils import StaticTuple


def main():
    var t = StaticTuple[Int, 3].__init__[1, 2]()
    print(t[0])
