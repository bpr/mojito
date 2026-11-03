# A read of a struct value parameter is a fresh value the parameter
# constant materializes into, so `var r = q` takes it and runs no copy
# constructor, as at the pin; copying the binding it initialized does. A
# struct with `@fieldwise_init` and a hand-written copy `__init__` is a
# parameter value too.


@fieldwise_init
struct Q(ImplicitlyCopyable):
    var a: Int

    def __init__(out self, *, copy: Self):
        print("copy Q", copy.a)
        self.a = copy.a

    def __deinit__(deinit self):
        print("del Q", self.a)


def take(var x: Q):
    print("take", x.a)


def f[q: Q]():
    var r = q
    print("G", r.a)
    var s = r
    print("H", s.a)
    var t = q
    print("T", t.a)
    var u = t
    print("U", t.a, u.a)


def h[q: Q]():
    take(q)
    take(q)
    print("h", q.a)


def main():
    f[Q(8)]()
    h[Q(4)]()
    print("end")
