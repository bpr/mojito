# A local `comptime` binding of an application is an alias of it, as a
# module constant is: it has no storage, and each read materializes a fresh
# value that dies after the read. A field read extracts from the
# application, and a nested `def` reads the alias without capturing it.


@fieldwise_init
struct Noisy(ImplicitlyCopyable, Writable):
    var v: Int

    def __init__(out self, *, copy: Self):
        self.v = copy.v
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("Noisy(", self.v, ")")


def make(n: Int) -> Noisy:
    return Noisy(n)


def label(n: Int) -> String:
    return String(t"n={n}")


def show(x: Noisy):
    print("show", x.v)


def take(var x: Noisy):
    print("take", x.v)


def count(x: Noisy) -> Int:
    return x.v


def gen[k: Int]():
    comptime s = make(k)
    comptime t = label(k)

    def inner():
        show(s)
        print(t)

    inner()
    var u = t
    u += "!"
    print(u, t)
    print("gen-end")


def main() raises:
    comptime s = make(3)
    print("start")
    print(s)
    show(s)
    take(s)
    var t = s
    t.v = 9
    print("mid", s.v, t.v)

    def inner():
        show(s)

    inner()
    for _ in range(2):
        show(s)
    try:
        show(s)
        raise Error("x")
    except:
        show(s)
    comptime for _ in range(2):
        show(s)
    print(comptime(count(s)))
    comptime c = count(s)
    print(c)
    print("end")
    gen[4]()
    gen[6]()
