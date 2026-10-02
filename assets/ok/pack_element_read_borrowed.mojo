# A read argument that is an element of a pack, a tuple, or an owning
# container is read where it lies, as a field is: neither `print` nor a
# `read` parameter runs the element's copy constructor.
struct Dup(Copyable, Writable):
    var n: Int

    def __init__(out self, n: Int):
        self.n = n

    def __init__(out self, *, copy: Self):
        self.n = copy.n
        print("copy", self.n)

    def __deinit__(deinit self):
        print("del", self.n)

    def write_to(self, mut writer: Some[Writer]):
        writer.write("D", self.n)

def g(d: Dup):
    print("g", d.n)

def show[*Ts: Writable](*a: *Ts):
    print("in")
    print(a[0])
    print(a[0], a[1])
    print("out")

def one[T: Writable](*a: T):
    print(a[0])

def main():
    var xs = [Dup(1)]
    print("built")
    print(xs[0])
    g(xs[0])
    var t = (Dup(5), 2)
    g(t[0])
    show(xs[0], 2)
    one(xs[0])
    print("end")
