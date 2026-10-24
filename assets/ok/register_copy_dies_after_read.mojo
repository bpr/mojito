# An owned register nothing takes dies after its last read, as upstream's
# lifetime check destroys an owned value at its death: a copy a generic body
# makes and only `print` reads is destroyed once `print` returns. A copy of
# a slot that dies right at it is the slot's move, so a pack element's copy
# bound to `x` and printed runs one copy initializer, not two.
struct D(Copyable, Writable):
    var v: Int

    def __init__(out self, v: Int):
        self.v = v

    def __init__(out self, *, copy: Self):
        self.v = copy.v + 10
        print("copy", self.v)

    def __deinit__(deinit self):
        print("del", self.v)

    def write_to(self, mut w: Some[Writer]):
        w.write("D", self.v)


def go[*Ts: Writable & Copyable & Deinitable](*a: *Ts):
    comptime for i in range(Ts.length):
        var x = a[i].copy()
        print(x)


def show[T: Writable & Copyable & Deinitable](x: T):
    var y = x.copy()
    print(y)
    print("after")


def main():
    go(D(1))
    print("mid")
    show(D(2))
    print("end")
