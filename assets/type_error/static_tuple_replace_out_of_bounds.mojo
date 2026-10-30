# `_replace[idx]` past the size fails its `comptime assert`.
# expect: function instantiation of `main` failed: constraint failed:
from std.utils import StaticTuple


def main():
    var t = StaticTuple[Int, 3](fill=1)
    print(t._replace[3](9)[0])
