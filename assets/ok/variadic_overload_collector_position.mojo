# Two overloads of a homogeneous variadic that agree on every parameter type
# and differ only in where the collector sits: a parameter after it is
# keyword-only, and its name is part of the callable's identity.


def route(a: Int, *rest: Int) -> Int:
    return 1


def route(*rest: Int, a: Int) -> Int:
    return 2


def main():
    print(route(1, 2))
    print(route(2, a=1))
