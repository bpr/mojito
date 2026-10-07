# A `comptime for` over tuple or struct elements that hold a `String` keeps
# one loop in a generic body's template, its variable a compile-time
# parameter of the element type, as at the pin: a named list, a literal
# display, a display over a parameter, and a struct whose field is a
# `String`. A parameter is no storage, so every run-time use of the variable
# materializes it afresh — a read argument, a copy, a transfer into a `var`
# parameter, a field read, a method receiver — in a static method, a nested
# loop, and a loop left by `break` alike. Output matches the pin.

comptime PAIRS = [(1, "a"), (2, "b")]


@fieldwise_init
struct Q(Copyable, Movable):
    var a: Int
    var s: String


comptime QS = [Q(1, "x"), Q(2, "y")]


def mk(n: Int) -> Q:
    return Q(n * 10, "m")


def show(t: Tuple[Int, String]):
    print("show", t[0], t[1])


def take(var t: Tuple[Int, String]):
    print("take", t[1])


def generic[n: Int]():
    comptime for p in PAIRS:
        print(p[0] + n, p[1])
        show(p)
        var q = p
        print(q[1])
        take(p)
    comptime for p in [(n, "c"), (4, "d")]:
        print(p[0], p[1])
    comptime for q in QS:
        print(q.a + n, q.s, q.s.upper())
    comptime for q in [Q(n, "z"), mk(n)]:
        print(q.a, q.s)
    comptime for p in PAIRS:
        comptime for q in QS:
            print(p[1], q.s, p[0] * q.a)
    comptime for p in PAIRS:
        if p[0] == n:
            break
        print("before break", p[1])


struct S[n: Int]:
    @staticmethod
    def show():
        comptime for p in PAIRS:
            print(p[1], p[0] + Self.n)


def main():
    generic[2]()
    S[100].show()
