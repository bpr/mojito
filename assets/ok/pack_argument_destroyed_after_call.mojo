# An argument a read `*args` collector gathers is lent, like a `read`
# parameter's: a local outlives the call it was last used in, and a temporary
# is destroyed once the call returns. A top-level `def`, a method, and a
# static method collect alike.
struct Noisy(Movable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("N", self.n)


@fieldwise_init
struct Box:
    var k: Int

    def show[*Ts: Writable](self, *a: *Ts):
        print("m-in", self.k)
        print(a[0])
        print("m-out")

    @staticmethod
    def stat[*Ts: Writable](*a: *Ts):
        print("s-in")
        print(a[0])
        print("s-out")


@fieldwise_init
struct Holder(Movable):
    var inner: Noisy


def show[*Ts: Writable](*a: *Ts):
    print("in")
    print(a[0])
    print("out")


def outer[*Ts: Writable](*a: *Ts):
    print("outer-in")
    show(*a)
    print("outer-out")


def plain(*a: Int):
    print("plain", a[0])


def lead[*Ts: Writable](x: Int, *a: *Ts):
    print("lead-in", x)
    print(a[1])
    print("lead-out")


def main():
    var y = Noisy(6)
    show(y, 5)
    print("after1")
    show(Noisy(7), 5)
    print("after2")
    var z = Noisy(8)
    var b = Box(1)
    b.show(z, 2)
    print("after3")
    var w = Noisy(9)
    Box.stat(w)
    print("after4")
    var v = Noisy(10)
    lead(1, 2, v)
    print("after5")
    var u = Noisy(11)
    show(u, 1)
    show(u, 2)
    plain(3, 4)
    var h = Holder(Noisy(12))
    show(h.inner)
    print("after6")
    var n = Noisy(13)
    outer(n, 6)
    print("after7")
    var xs: List[Int] = [1, 2]
    var s = String("str")
    show(xs, s)
    xs.append(3)
    print(xs, s)
    show(String("tmp"), [7, 8])
    print("done")
