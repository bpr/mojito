# `Self.<name>` reads only a parameter the struct declares: beside a
# readable origin-mutability binder `Self.mut`, `Self.nope` stays an
# unknown member of `Self`, as at the pin.
# expect: is not a type parameter of the enclosing struct
struct Q[mut: Bool, //, origin: Origin[mut=mut]]:
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def g(self) -> Bool:
        return Self.nope


def main():
    var x = 1
    print(Q[origin_of(x)](4).g())
