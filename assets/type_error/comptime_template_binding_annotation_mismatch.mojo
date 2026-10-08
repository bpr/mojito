# expect: type mismatch for comptime 'm': expected Int, found Bool
# A local `comptime` binding in a generic body's `comptime for` whose
# annotation is not its value's type is rejected, as at the pin: a `Bool`
# parameter expression does not convert to `Int`.
struct S[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        comptime for i in range(Self.n):
            comptime m: Int = i > 0
            print(m)


def main():
    S[2]().f()
