# `len` of a homogeneous collector, a tuple, or a value pack reads the
# storage where it lies, as the pin's `len[T: Sized](value: T)` takes it by
# `read`: taking the length runs no copy constructor.
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


def count(*a: IC) -> Int:
    return len(a)


def count_owned(var *a: IC) -> Int:
    return len(a)


def count_generic[T: Copyable](*a: T) -> Int:
    return len(a)


def count_ints(*a: Int) -> Int:
    return len(a) + a[0]


def main():
    var p = IC(1)
    var q = IC(2)
    print(count(p, q))
    print(count_generic(p, q, p))
    print(count_owned(IC(3)))
    print(count_ints(10, 20))
    var t = (IC(4), IC(5))
    print(len(t))
    print("end")
