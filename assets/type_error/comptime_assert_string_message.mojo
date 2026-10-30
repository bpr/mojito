# expect: function instantiation of `S.check` failed: constraint failed: size 2 is too small
# A `comptime assert` message may be any compile-time `String` expression over
# the parameters in scope, here a struct's own, evaluated for the instance the
# assertion fails.

struct S[size: Int]:
    def __init__(out self):
        pass

    def check(self):
        comptime assert Self.size >= 4, (
            String("size ") + String(Self.size) + " is too small"
        )
        print(Self.size)


def main():
    S[4]().check()
    S[2]().check()
