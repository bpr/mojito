# A method's signature may call a method its struct declares below it, and
# a method of a struct declared below its own, as upstream resolves a
# signature's calls on demand: `neg` reads `sub`, `a` reads `b` reads `c`,
# and `V.a` constructs and calls `U`.


struct W[n: Int]:
    def __init__(out self):
        pass

    def neg(self) -> type_of(self.sub(self)):
        return self.sub(self)

    def sub(self, o: W[_]) -> W[Self.n - o.n + 7]:
        return W[Self.n - o.n + 7]()

    def a(self) -> type_of(self.b()):
        return self.b()

    def b(self) -> type_of(self.c()):
        return self.c()

    def c(self) -> W[Self.n + 1]:
        return W[Self.n + 1]()


struct V:
    def __init__(out self):
        pass

    def a(self) -> type_of(U().b()):
        return U().b()


struct U:
    def __init__(out self):
        pass

    def b(self) -> type_of(W[2]().a()):
        return W[2]().a()


def main():
    print(W[3]().neg().n, W[3]().a().n, V().a().n)
