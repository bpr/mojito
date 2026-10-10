# Trait defaults are inherited per overload: a conformer that spells one
# overload still inherits the other.
trait T:
    def d(self) -> Int:
        return 1

    def d(self, x: Int) -> Int:
        return x

struct S(T):
    def __init__(out self):
        pass

struct U(T):
    def __init__(out self):
        pass

    def d(self) -> Int:
        return 100

def through[X: T](value: X) -> Int:
    return value.d() + value.d(1000)

def main():
    print(S().d(), S().d(5))
    print(U().d(), U().d(6))
    print(through(S()), through(U()))
