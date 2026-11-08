# Pinned against Mojo 1.3.0.dev2026110105 (2026-11-01): a trivially
# register-passable value read out of an owner and passed to a `read`
# parameter, the receiver of a `read self` method included, is loaded at
# call setup in parameter order, receiver first, so an owner whose last use
# it is dies before the call runs; a subscript getter's and `__contains__`'s
# receiver, plain, multi-index, or slice, is loaded the same way. A
# `mut self` receiver and a memory-only one keep their owner alive through
# the call.
# Expected: del 1 / del 2 / add / 3 / del 1 / del 2 / add / 3 / del 1 /
# del 2 / op / 3 / del 5 / add / after / del 3 / one / 2 / del 2 / del 1 /
# free / 3 / del 2 / add / del 1 / add / 3 / bump / del 3 / after / show 5 /
# vdel 4 / after / del 6 / getitem / 3 / del 7 / getitem2 / 4 / del 8 /
# getslice / 1 / del 9 / contains / True / del 10 / contains / False
@fieldwise_init
struct P(TrivialRegisterPassable):
    var v: Int

    def add(self, other: P) -> P:
        print("add")
        return P(self.v + other.v)

    def __add__(self, other: P) -> P:
        print("op")
        return P(self.v + other.v)

    def __getitem__(self, i: Int) -> Int:
        print("getitem")
        return self.v + i

    def __getitem__(self, i: Int, j: Int) -> Int:
        print("getitem2")
        return self.v + i + j

    def __getitem__(self, s: Slice) -> Int:
        print("getslice")
        return self.v

    def __contains__(self, x: Int) -> Bool:
        print("contains")
        return x == self.v

    def bump(mut self):
        print("bump")
        self.v += 1


@fieldwise_init
struct M(Movable):
    var v: Int

    def show(self):
        print("show", self.v)


@fieldwise_init
struct W(Movable):
    var p: P
    var tag: Int

    def __deinit__(deinit self):
        print("del", self.tag)


@fieldwise_init
struct V(Movable):
    var m: M
    var tag: Int

    def __deinit__(deinit self):
        print("vdel", self.tag)


def one(p: P) -> P:
    print("one")
    return p


def free(x: P, y: P) -> P:
    print("free")
    return P(x.v + y.v)


def method():
    var a = W(P(1), 1)
    var b = W(P(2), 2)
    var r = a.p.add(b.p)
    print(r.v)


def declared_first():
    var b = W(P(2), 2)
    var a = W(P(1), 1)
    var r = a.p.add(b.p)
    print(r.v)


def operator():
    var a = W(P(1), 1)
    var b = W(P(2), 2)
    var r = a.p + b.p
    print(r.v)


def receiver_alone():
    var a = W(P(1), 5)
    _ = a.p.add(P(0))
    print("after")


def argument():
    var b = W(P(2), 3)
    var r = one(b.p)
    print(r.v)


def free_function():
    var a = W(P(1), 1)
    var b = W(P(2), 2)
    var r = free(b.p, a.p)
    print(r.v)


def chained():
    var a = W(P(1), 1)
    var b = W(P(2), 2)
    var r = P(0).add(b.p).add(a.p)
    print(r.v)


def mutating():
    var a = W(P(1), 3)
    a.p.bump()
    print("after")


def memory_only():
    var a = V(M(5), 4)
    a.m.show()
    print("after")


def subscripts():
    var a = W(P(1), 6)
    var r = a.p[2]
    print(r)
    var b = W(P(1), 7)
    var s = b.p[1, 2]
    print(s)
    var c = W(P(1), 8)
    var t = c.p[0:2]
    print(t)
    var d = W(P(1), 9)
    var u = 1 in d.p
    print(u)
    var e = W(P(1), 10)
    var w = 1 not in e.p
    print(w)


def main():
    method()
    declared_first()
    operator()
    receiver_alone()
    argument()
    free_function()
    chained()
    mutating()
    memory_only()
    subscripts()
