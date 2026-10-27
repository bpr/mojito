# A struct's `comptime` member that applies a function is an application
# over the struct's parameters, which each instance evaluates: read through
# `Self`, through an instance and the unparameterized struct, in a field's
# type, a `comptime if`, a later member, and a compile-time binding.
def f(n: Int) -> Int:
    return n + 1


def even(n: Int) -> Bool:
    return n % 2 == 0


struct Box[n: Int]:
    def __init__(out self):
        pass

    def get(self) -> Int:
        return Self.n


struct S[k: Int]:
    comptime V = f(Self.k)
    comptime W = Self.V * 10
    comptime E = even(Self.V)
    var b: Box[Self.V]

    def __init__(out self):
        self.b = Box[Self.V]()

    def get(self) -> Int:
        return Self.V

    def show(self):
        comptime if Self.E:
            print("even", Self.W, self.b.get())
        else:
            print("odd", Self.W, self.b.get())


struct T:
    comptime K = f(10)


def through[n: Int]() -> Int:
    return S[n].V


def main():
    print(S[3]().get())
    S[3]().show()
    S[4]().show()
    print(S[5].V, T.K, through[7]())
    comptime w = S[5].W
    print(w)
    print(Box[S[2].V]().get())
