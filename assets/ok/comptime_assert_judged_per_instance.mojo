# The elaborator judges a `comptime assert` per reached instance, as the pin
# does: an untaken `comptime if` arm's assertion, an uninstantiated function's,
# and an uncalled method of a constructed instance are never judged, and a
# `comptime for` judges its assertion once per iteration.

def arm[n: Int]():
    comptime if n > 5:
        comptime assert n > 100, "big"
    print(n)


def never():
    comptime assert False, "never"


def never_generic[n: Int]():
    comptime assert False, "never"


struct S[n: Int]:
    def __init__(out self):
        pass

    def m(self):
        comptime assert Self.n > 0, "positive"
        print(Self.n)


def main():
    arm[3]()
    _ = S[0]()
    S[2]().m()
    comptime for i in range(3):
        comptime assert i < 3, "small i"
        print(i)
