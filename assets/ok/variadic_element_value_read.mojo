# A value read of a homogeneous collector's element copies that element
# alone, as the pin does: `var x = a[1]` runs one copy constructor, a field
# read through an element copies nothing, and a runtime index copies only the
# element it names.
struct IC(ImplicitlyCopyable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("I", self.n)


def h(*a: IC):
    print("h")
    var x = a[1]
    print(x)
    print("h end")


def fields(*a: IC) -> Int:
    return a[0].n + a[1].n


def each(*a: IC):
    for i in range(2):
        var x = a[i]
        print(x)


def total(*a: Int) -> Int:
    var s = a[0]
    s += a[1]
    return s


def main():
    var p = IC(1)
    var q = IC(2)
    h(p, q)
    print(fields(p, q))
    each(p, q)
    print(total(3, 4))
    print("end")
