# Pin gap probe (Mojo 1.2.0.dev2026092105): a module `comptime` list a call
# builds, iterated by a struct method's `comptime for`. The pin prints `0`,
# `1`, `2`. Mojito stops with "function instantiation in parameter domain
# that recursively requires itself: the initializer of 'ML' reads 'ML'"; the
# same loop in a `def` runs. Roadmap R501.
def mk(n: Int) -> List[Int]:
    var l = List[Int]()
    for i in range(n):
        l.append(i)
    return l^


comptime ML = mk(3)


struct S:
    def __init__(out self):
        pass

    def m(self):
        comptime for x in ML:
            print(x)


def main():
    S().m()
