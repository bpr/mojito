# A compile-time-keyed generic `def` overloaded with a plain one. Only the
# keyed declaration is a template: the plain overload is not specialized, is
# walked for its own template uses, and survives the program rebuild — which
# decides per declaration rather than by name.


def kind[T: Copyable](a: T) -> Int:
    comptime if T == Int:
        return 1
    else:
        return 10


def kind(a: Int, b: Int) -> Int:
    return 2


# Same arity and same parameter name as the keyed overload, so only the
# parameter types tell the two apart.
def kind(a: String) -> Int:
    return 3


def through(a: Int) -> Int:
    # The plain overload reached from another body, so its survival is
    # observable rather than incidental.
    return kind(a, a) + kind(a)


def main():
    print(kind(3))
    print(kind(3, 4))
    print(kind(True))
    print(kind(String("x")))
    print(through(7))
