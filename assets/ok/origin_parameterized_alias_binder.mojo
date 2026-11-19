# A binder typed by a parameterized alias of `Origin` is an origin binder,
# as upstream resolves the alias like any other type: a fixed mutability, a
# sibling `mut` binder, and a struct's binder.

comptime O[m: Bool] = Origin[mut=m]


def bump[o: O[True]](ref[o] x: Int):
    x += 1


def read[mm: Bool, //, o: O[mm]](ref[o] x: Int) -> Int:
    return x


struct Holder[o: O[False]]:
    var p: Pointer[Int, Self.o]

    def __init__(out self, p: Pointer[Int, Self.o]):
        self.p = p


def main():
    var a = 1
    bump(a)
    print(a)
    print(read(a))
    var h = Holder(Pointer(to=a))
    print(h.p[])
