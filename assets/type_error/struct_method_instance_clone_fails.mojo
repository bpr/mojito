# expect: function instantiation of `S.f
# A struct method's template is instantiated per reached instance; the
# instance whose loop reads past a literal display fails, as at the pin,
# rather than trapping at run time.
struct S[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        comptime for i in range(Self.n):
            comptime q = [1, 2][i]
            print(q)


def main():
    S[2]().f()
    S[5]().f()
