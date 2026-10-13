# A call through a trait bound passes its arguments by the requirement's
# declared conventions: a `read` `Int` is loaded at call setup, so an owner
# whose last use is that load dies before the call. A `mut` argument keeps
# its owner alive through the call.


trait Sh:
    def sh(self, x: Int) -> Int:
        ...

    def bump(self, mut x: Int) -> Int:
        ...

    def go(self) -> Int:
        var b = W(41)
        return self.sh(b.t)


trait Sh2(Sh):
    def other(self) -> Int:
        ...


trait Ad:
    def __getitem__(self, i: Int) -> Int:
        ...


struct W(Movable):
    var t: Int

    def __init__(out self, t: Int):
        self.t = t

    def __deinit__(deinit self):
        print("del", self.t)


struct S(Ad, Copyable, Movable, Sh):
    def __init__(out self):
        pass

    def sh(self, x: Int) -> Int:
        print("sh")
        return x

    def bump(self, mut x: Int) -> Int:
        print("bump")
        x += 1
        return x

    def __getitem__(self, i: Int) -> Int:
        print("get")
        return i


struct S2(Copyable, Movable, Sh2):
    def __init__(out self):
        pass

    def sh(self, x: Int) -> Int:
        print("sh2")
        return x

    def bump(self, mut x: Int) -> Int:
        return x

    def other(self) -> Int:
        return 0


def run[T: Sh](s: T):
    var b = W(31)
    var q = s.sh(b.t)
    print(q)


def run_refined[T: Sh2](s: T):
    var b = W(51)
    var q = s.sh(b.t)
    print(q)


def run_subscript[T: Ad](s: T):
    var c = W(32)
    var r = s[c.t]
    print(r)


def run_mut[T: Sh](s: T):
    var b = W(61)
    var q = s.bump(b.t)
    print(q)


def main():
    run(S())
    print(S().go())
    run_refined(S2())
    run_subscript(S())
    run_mut(S())
