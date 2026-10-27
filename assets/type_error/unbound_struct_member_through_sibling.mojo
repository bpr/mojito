# A member read through a struct left unbound may not depend on the
# struct's parameters through a sibling it reads either.
# expect: cannot access comptime member 'b' with unbound parameter 'S.T'
struct S[T: AnyType]:
    comptime a = Self.T
    comptime b = Self.a


def main():
    comptime m = S.b
