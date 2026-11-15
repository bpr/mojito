# An arithmetic `where` operand compiles on every declaration that opens its
# parameters: a generic alias, a predicate alias's body and argument, a struct
# or trait `comptime` member, a static method, a struct header, and a nested
# `def` reading an outer parameter.
comptime Next[n: Int, m: Int]: Int where n + 1 == m = m
comptime IsNext[n: Int, m: Int] = n + 1 == m
comptime IsNext2[n: Int, m: Int] = IsNext[n + 1, m]


def doubled[n: Int, m: Int]() where IsNext[n * 2, m]:
    print("doubled", n, m)


def chained[n: Int, m: Int]() where IsNext2[n, m]:
    print("chained", n, m)


trait Stepped:
    comptime C[n: Int, m: Int]: Int where n + 1 == m


struct Step(Stepped):
    comptime C[n: Int, m: Int]: Int where n + 1 == m = m

    def __init__(out self):
        pass


struct Pair[n: Int, m: Int]:
    comptime Gap[k: Int]: Int where Self.n + k == Self.m = k

    def __init__(out self):
        pass

    @staticmethod
    def gap[k: Int]() where Self.n + k == Self.m:
        print("gap", k)


struct Half[n: Int] where n * 2 == 4:
    def __init__(out self):
        pass


def outer[k: Int]():
    def inner[n: Int]() where n + k == 3:
        print("inner", n, k)

    inner[3 - k]()


def main():
    print("alias", Next[1, 2])
    doubled[1, 3]()
    chained[1, 3]()
    print("member", Pair[1, 3].Gap[2])
    print("trait member", Step.C[6, 7])
    Pair[1, 3].gap[2]()
    _ = Half[2]()
    print("header ok")
    outer[1]()
