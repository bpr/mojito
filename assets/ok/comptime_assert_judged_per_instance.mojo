# The elaborator judges a `comptime assert` per reached instance, as the pin
# does: an untaken `comptime if` arm's assertion, an uninstantiated function's,
# and an uncalled method of a constructed instance are never judged, and a
# `comptime for` judges its assertion once per iteration. A message that is
# no string literal (a t-string, a `String` expression, a call) is evaluated
# only for an instance whose assertion fails, so a holding one costs nothing.

def describe(n: Int) -> String:
    return String("described ") + String(n)


def arm[n: Int]():
    comptime if n > 5:
        comptime assert n > 100, "big"
    comptime assert n > 0, t"n is {n}"
    comptime assert n < 10, String("n is ") + String(n)
    comptime assert n != 4, "lit" + "eral"
    comptime assert n != 5, describe(n)
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
