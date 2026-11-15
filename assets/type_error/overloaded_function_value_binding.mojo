# A bare overload set selects no candidate without a contextual type, so an
# inferred binding cannot form a reference to it.
# expect: cannot form a reference to overloaded declaration of 'f'
def f(x: Int) -> Int:
    return x


def f(x: String) -> Int:
    return 1


def main():
    var g = f
    print(g(1))
