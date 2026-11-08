# expect: expected U, found IntLiteral[3]
# A parameter value converts through an `@implicit` constructor as a value
# does: an integer literal reaches one taking `UInt8` only through a second
# conversion, so the binding is rejected.
struct U(ImplicitlyCopyable):
    var v: UInt8

    @implicit
    def __init__(out self, value: UInt8):
        self.v = value


def f[w: U]():
    print(w.v)


def main():
    f[3]()
