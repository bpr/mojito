# A `comptime for` over a struct's value parameter in its methods, whose
# body binds an annotated `comptime` over the index: the method's template
# decides the loop per instance, as at the pin.


struct S[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        comptime for i in range(Self.n):
            comptime m: Int = i * Self.n
            print(m)

    def g(self):
        comptime if Self.n > 2:
            comptime for i in [Self.n, 1]:
                comptime m: Int = i + 10
                print(m)
        else:
            print("small")


def main():
    var s = S[2]()
    s.f()
    s.g()
    var t = S[3]()
    t.f()
    t.g()
