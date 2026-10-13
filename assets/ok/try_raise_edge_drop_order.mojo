# A raise inside a `try` destroys the body's live values in reverse
# declaration order: an operator, method, free, or constructor call over
# temporaries, and a named local beside them.


struct S(Movable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __add__(self, other: S) raises -> S:
        if other.v > 5:
            raise Error("big")
        return S(self.v + other.v)

    def add(self, other: S) raises -> S:
        if other.v > 5:
            raise Error("big")
        return S(self.v + other.v)

    def __deinit__(deinit self):
        print("del", self.v)


struct P(Movable):
    var a: S
    var b: S

    def __init__(out self, a: S, b: S, n: Int) raises:
        if n > 0:
            raise Error("boom")
        self.a = S(a.v)
        self.b = S(b.v)


def g(a: S, b: S) raises -> S:
    if b.v > 5:
        raise Error("big")
    return S(a.v + b.v)


def may(x: Int) raises -> Int:
    if x > 5:
        raise Error("big")
    return x


def look(a: S, b: S):
    print("look", a.v, b.v)


def main():
    try:
        var x = S(1) + S(9)
        print(x.v)
    except e:
        print("caught", e)
    try:
        var x = S(1).add(S(9))
        print(x.v)
    except e:
        print("caught", e)
    try:
        var x = g(S(1), S(9))
        print(x.v)
    except e:
        print("caught", e)
    try:
        var a = S(5)
        var x = g(S(1), S(9))
        print(a.v, x.v)
    except e:
        print("caught", e)
    try:
        var p = P(S(3), S(4), 1)
        print(p.a.v)
    except e:
        print("caught", e)
    try:
        var c = S(3)
        var d = S(4)
        var n = may(1)
        look(c, d)
        print(n)
    except e:
        print("caught", e)
