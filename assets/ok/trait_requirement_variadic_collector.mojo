# A trait requirement may declare a `*args` collector, homogeneous or a
# pack, read or `var`; a call or a spread through the bound dispatches to
# the witness, which may rename the collector.


trait Taker:
    def take[*Ts: Writable](self, *a: *Ts):
        ...


struct S(Taker):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def take[*Ts: Writable](self, *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i], end=" ")
        print(self.n)


def spread[S: Taker, *Ts: Writable](s: S, *a: *Ts):
    s.take(*a)


def direct[S: Taker](s: S):
    s.take(1, "two", 3.5)


trait Summer:
    def sum(self, *xs: Int, scale: Int = 10) -> Int:
        ...


struct A(Summer):
    def __init__(out self):
        pass

    def sum(self, *ys: Int, scale: Int = 1) -> Int:
        var t = 0
        for y in ys:
            t += y
        return t * scale


def total[T: Summer](t: T) -> Int:
    return t.sum(1, 2, 3)


def scaled[T: Summer](t: T) -> Int:
    return t.sum(1, 2, 3, scale=2)


trait Shower:
    def show[*Ts: Writable](self, *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i], end=",")
        print()


struct B(Shower):
    def __init__(out self):
        pass


def show_all[T: Shower, *Ts: Writable](t: T, *a: *Ts):
    t.show(*a)


trait Eater:
    def eat[*Ts: Copyable & Writable](self, var *a: *Ts):
        ...


struct C(Eater):
    def __init__(out self):
        pass

    def eat[*Ts: Copyable & Writable](self, var *a: *Ts):
        comptime for i in range(a.__len__()):
            print(a[i])


def feed[T: Eater](t: T):
    t.eat(String("s"), 3)


def main():
    var s = S(7)
    s.take(1, 2)
    spread(s, "x", 4)
    direct(s)
    print(total(A()), scaled(A()), A().sum(1, 2))
    B().show(1, "a")
    show_all(B(), 2.5, True)
    feed(C())
