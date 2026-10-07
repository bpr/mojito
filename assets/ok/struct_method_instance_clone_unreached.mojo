# A struct method whose instance would fail to instantiate is accepted while
# no reachable call needs that instance: neither an uncalled method nor a
# call inside an uncalled function instantiates it, as at the pin.
struct S[n: Int]:
    def __init__(out self):
        pass

    def f(self):
        comptime for i in range(Self.n):
            comptime q = [1, 2][i]
            print(q)

    def g(self):
        print(Self.n)


def unused():
    S[5]().f()


def main():
    S[5]().g()
    S[2]().f()
