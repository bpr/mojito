# expect: cannot form a reference to overloaded declaration of 'f'
# An overload set named bare selects no declaration, so no compile-time
# value is formed from it.


def f(x: Int) -> Int:
    return x


def f(x: String) -> Int:
    return 1


comptime h = f


def main():
    print(h(3))
