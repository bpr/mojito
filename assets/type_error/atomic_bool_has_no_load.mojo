# expect: no overload matches
# `Bool` is no `Scalar[dtype]`, so no `Atomic[Bool]` method's `where`
# clause holds.
from std.atomic import Atomic


def main():
    var a = Atomic[Bool](True)
    print(a.load())
