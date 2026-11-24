# expect: expected Bool, found Int
# A method binder's `mut` reading a non-`Bool` struct parameter through
# `Self` is rejected where the list is classified, as upstream builds the
# argument of `Origin`'s `mut` and checks it against `Bool`.


struct S[mut: Bool, n: Int]:
    var x: Int

    def __init__(out self):
        self.x = 1

    def g[t: Origin[mut=Self.n]](self, v: Int) -> Int:
        return v


def main():
    var s = S[True, 3]()
    var a = 4
    print(s.g(a))
