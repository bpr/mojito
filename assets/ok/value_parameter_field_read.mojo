# A field read of a struct value parameter is the parameter's field alone:
# `q.a` is a constant, `q.s` and `q.i` materialize only the field, and only
# `q.get()` and `var r = q` build the whole `Q`.


@fieldwise_init
struct In(ImplicitlyCopyable):
    var v: Int

    def __deinit__(deinit self):
        print("del In", self.v)


@fieldwise_init
struct Q(ImplicitlyCopyable):
    var a: Int
    var i: In
    var s: String

    def __deinit__(deinit self):
        print("del Q", self.a)

    def get(self) -> Int:
        return self.a


@fieldwise_init
struct N(ImplicitlyCopyable):
    var a: Int
    var i: In

    def __deinit__(deinit self):
        print("del N", self.a)


def show(x: In):
    print("show", x.v)


def ints[n: N]():
    print(n.a)
    print(n.i.v)


def f[q: Q]():
    print("A", q.a)
    print("B", q.i.v)
    print("C", q.s)
    show(q.i)
    var j = q.i
    print("E", j.v)
    print("L", q.s.byte_length())
    var t = q.s
    t += "y"
    print("T", t)
    print("F", q.get())
    var r = q
    print("G", r.a)


def main():
    ints[N(8, In(9))]()
    f[Q(8, In(5), "x")]()
    print("end")
