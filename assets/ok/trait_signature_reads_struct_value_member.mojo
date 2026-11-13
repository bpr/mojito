# A trait's method signature reads a struct's value member, through a
# parameterized application or a plain struct, declared before or after the
# trait: the trait's check waits for the struct's members to register.


struct P[A: AnyType, B: AnyType]:
    comptime n = 3


struct Q[k: Int]:
    def __init__(out self):
        pass

    def size(self) -> Int:
        return Self.k


trait Reads:
    def get(self, x: Q[P[Int, Bool].n]) -> Int:
        ...

    def other(self, x: Q[R.m]) -> Int:
        ...


struct S(Reads):
    def __init__(out self):
        pass

    def get(self, x: Q[P[Int, Bool].n]) -> Int:
        return x.size() * 10

    def other(self, x: Q[5]) -> Int:
        return x.size() + 1


def use[T: Reads](t: T) -> Int:
    return t.get(Q[3]()) + t.other(Q[5]())


struct R:
    comptime m = 5


def main():
    print(use(S()))
