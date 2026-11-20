# A struct's origin-mutability binder is read through `Self`: a method's
# bare `mut` names no value in scope, as at the pin.
# expect: Undefined variable 'mut'
struct Q[mut: Bool, //, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def g(self) -> Bool:
        return mut


def main():
    var x = 1
    print(Q[origin_of(x)](4).g())
