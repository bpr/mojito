# A member read through a struct left unbound may not depend on the
# struct's parameters, and a default does not bind one.
# expect: cannot access comptime member 'Y' with unbound parameter 'D.T'
struct D[T: AnyType = Int]:
    comptime Y = Self.T


def main():
    var x: D.Y = 3
    print(x)
