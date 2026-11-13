# A struct's `comptime` type member read through an application over a
# symbolic parameter (`T[t].current`, `T[t].boxed`): the member's value with
# `Self` bound to the instance, as a static call's receiver, a member read's
# base, a construction, and a compile-time alias.


struct Box[n: Int]:
    def __init__(out self):
        pass

    def __init__(out self, x: Int):
        pass

    def get(self) -> Int:
        return Self.n + 10


struct T[n: Int]:
    comptime current = Self
    comptime boxed = Box[Self.n + 1]
    comptime twice = T[Self.n * 2]

    @staticmethod
    def answer() -> Int:
        return Self.n


def f[t: Int]():
    print(T[t].current.answer())
    print(T[t].boxed.n)
    print(T[t].boxed(10).get())
    print(T[n=t].twice.answer())
    comptime C = T[t + 1].current
    print(C.answer())


def main():
    f[3]()
