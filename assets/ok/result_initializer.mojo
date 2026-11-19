# An `__init__` without `out self` that declares its result type is the
# initializer whose `out self` is that type, following its parameters: the
# construction infers the struct's parameters from the result, `Self.n`
# reads them in the body, and an `@implicit` one converts an argument into
# the type its signature computes.


struct W[n: Int]:
    def __init__(out self):
        pass

    @implicit
    def __init__(x: W[_]) -> W[x.n + 1]:
        print("Self.n =", Self.n)
        return {}


def show(w: W[3]):
    print(w.n)


def main():
    var w = W(W[2]())
    print(w.n)
    show(W[2]())
