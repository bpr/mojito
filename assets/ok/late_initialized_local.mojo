# A local declared without an initializer is initialized by its first
# assignment: in `main`, in both arms of an `if`, conditionally, never, and
# per iteration of a loop.


@fieldwise_init
struct S(Movable):
    var x: Int

    def __deinit__(deinit self):
        print("del", self.x)


def pick(c: Bool) -> Int:
    var a: S
    if c:
        a = S(1)
    else:
        a = S(2)
    print("picked")
    return a.x


def unused():
    var a: S
    print("unused")


def maybe(c: Bool):
    var a: S
    if c:
        a = S(5)
    print("maybe end")


def looped():
    for i in range(3):
        var a: S
        a = S(10 + i)
        print("it", a.x)


def reassign():
    var a: S
    a = S(20)
    a = S(21)
    print(a.x)


def main():
    var n: Int
    n = 7
    print(n)
    var w: String
    w = "late"
    print(w)
    print(pick(True))
    print(pick(False))
    unused()
    maybe(True)
    maybe(False)
    looped()
    reassign()
