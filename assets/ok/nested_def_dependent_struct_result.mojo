# A nested `def` whose result type computes over an enclosing parameter
# (`StaticTuple[Int, 2 * length]`) binds that parameter before its result
# unifies, called at run time or by a local `comptime`.

from std.utils import StaticTuple


def outer[length: Int]():
    def inner() -> StaticTuple[Int, 2 * length]:
        return StaticTuple[Int, 2 * length](fill=length)

    var t = inner()
    comptime c = inner()
    print(len(t), t[0], c[2 * length - 1])


def main():
    outer[4]()
    outer[2]()
