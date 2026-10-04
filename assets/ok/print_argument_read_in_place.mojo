# `print` reads its arguments in place: a named value of a type parameter is
# not copied, and an owned temporary is destroyed once `print` returns.
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


def f[T: ImplicitlyCopyable & Writable & Deinitable](x: T):
    var y = x
    print(y)
    print(x.copy())
    print("f end")


def g(x: IC):
    print(x)
    print(x.copy())
    print("g end")


def main():
    var p = IC(1)
    f(p)
    g(p)
    print(IC(5))
    print("end")
