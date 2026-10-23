# Each raising call is its own raise edge, as upstream's `addErrorRegions`
# makes it: the edge destroys what it holds, the values still live past the
# call and unread by the handler, whether the raise reaches a `try` or
# leaves the function, and a value ended before the call is not held there.


struct S(Movable):
    var tag: Int

    def __init__(out self, tag: Int):
        self.tag = tag

    def __deinit__(deinit self):
        print("del", self.tag)


def fail(x: Int) raises:
    if x > 0:
        raise Error("boom")


def take(var a: S, var b: S, n: Int):
    print("take", a.tag, b.tag, n)


def boom(n: Int) raises -> Int:
    raise Error("bang")


def leaves() raises:
    var a = S(1)
    fail(1)
    print(a.tag)


def borrows(a: S) raises -> Int:
    var b = S(5)
    fail(a.tag)
    return b.tag


def inner() raises:
    var t = S(7)
    fail(1)
    print(t.tag)


def outer() raises:
    var u = S(8)
    inner()
    print(u.tag)


@always_inline("nodebug")
def inlined() raises:
    var t = S(9)
    fail(1)
    print(t.tag)


def operands() raises:
    take(S(2), S(3), boom(2))


def main():
    try:
        var a = S(1)
        fail(0)
        print(a.tag)
        var b = S(2)
        fail(1)
        print(b.tag)
    except e:
        print("caught", e)
    try:
        leaves()
    except e:
        print("caught", e)
    var x = S(4)
    try:
        var y = S(2)
        var n = borrows(y)
        print(n, x.tag)
    except e:
        print("caught", e)
    try:
        outer()
    except e:
        print("caught", e)
    try:
        var k = S(3)
        inlined()
        print(k.tag)
    except e:
        print("caught", e)
    try:
        operands()
    except e:
        print("caught", e)
    print("end")
