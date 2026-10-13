# A local declared without an initializer is initialized by its first
# assignment: in `main`, in both arms of an `if`, conditionally, never, per
# iteration of a loop, and inside a `try` — where a local declared outside
# the body survives the body's exit and one declared inside it does not.


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


def may(x: Int) raises -> Int:
    if x > 5:
        raise Error("big")
    return x


def in_try():
    var a: S
    try:
        a = S(may(3))
    except e:
        print("caught", e)
        return
    print("after", a.x)


def in_try_loop():
    var a: S
    for i in range(3):
        try:
            a = S(40 + may(i))
            break
        except e:
            print("caught", e)
            return
    print("loop end")


def declared_in_try():
    try:
        var a: S
        a = S(may(5))
        print("inner", a.x)
    except e:
        print("caught", e)
    print("declared end")


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
    in_try()
    in_try_loop()
    declared_in_try()
