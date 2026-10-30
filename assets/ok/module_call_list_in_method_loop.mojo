# A module `comptime` list a call builds, iterated by a struct method's
# `comptime for`: the method's compile-time evaluation reaches the list's
# initializer without the struct's other constructors joining its closure.
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
