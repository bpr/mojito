# A local `comptime` struct binding in a plain `def` is a parameter, not
# storage: field reads are constants and destroy nothing, and each run-time
# use of the whole value materializes its own temporary.
struct N(ImplicitlyCopyable):
    var a: Int

    def __init__(out self, a: Int):
        self.a = a

    def __deinit__(deinit self):
        print("del", self.a)


def show(n: N):
    print("show", n.a)


def main():
    comptime c = N(8)
    print(c.a)
    print(c.a)
    show(c)
    var d = c
    d.a = 9
    print(d.a)
    print(c.a)
