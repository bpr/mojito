# expect: function instantiation of `S.f
# A struct method keyed by the struct's parameter is instantiated per
# instance; the instance a reached call needs fails, as at the pin, rather
# than trapping at run time.
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
